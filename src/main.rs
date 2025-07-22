use actix_web::rt::{spawn, time};
use actix_web::{App, HttpServer};
use chrono::{DateTime, FixedOffset, Utc};
use clipboard::{ClipboardContext, ClipboardProvider};
use hashbrown::HashSet;
use rdev::{simulate, EventType, Key};
use reqwest::Error as ReqwestError;
use rodio::{Decoder, OutputStream, Sink};
use serde::Deserialize;
use serialport::{self};
use std::io::Cursor;
use std::io::{self, Read};
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const COMPILED_AT: &str = env!(
    "COMPILED_AT",
    "COMPILED_AT environment variable not set by build.rs"
);
const RUST_COMPILER_VERSION: &str = env!(
    "RUST_COMPILER_VERSION",
    "RUST_COMPILER_VERSION environment variable not set by build.rs"
);
// Global state to store the latest valid barcode and server startup time
lazy_static::lazy_static! {
    static ref STARTUP_TIME: DateTime<FixedOffset> = {
        let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +03:00 Istanbul
        Utc::now().with_timezone(&timezone_offset)
    };
    static ref SERVER_VALID_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    static ref PRINTED_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());

    //https://ttsmaker.com/tr
    static ref SUCCESS_SOUND: &'static [u8] = include_bytes!("../assets/sounds/success.wav");
    static ref ERROR_SOUND: &'static [u8] = include_bytes!("../assets/sounds/error.wav");   
    static ref ERROR_TEST_SOUND: &'static [u8] = include_bytes!("../assets/sounds/error_test.wav");
    static ref ERROR_BARCODE_SOUND: &'static [u8] = include_bytes!("../assets/sounds/error_barcode.wav");
}

#[derive(Deserialize)]
struct Packet {
    barcode: String,
    status: String,
}

async fn fetch_packets() -> Result<Vec<Packet>, ReqwestError> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://172.22.5.196:8080/packets")
        .send()
        .await?
        .json::<Vec<Packet>>()
        .await?;
    Ok(response)
}

async fn collect_latest_barcode() -> Result<(), Box<dyn std::error::Error>> {
    let packets = fetch_packets().await?;
    let mut server_valid_barcodes_guard = SERVER_VALID_BARCODES.lock().unwrap();
    server_valid_barcodes_guard.clear();
    for packet in packets {
        if packet.status == "OK" && packet.barcode.len() == 18 {
            server_valid_barcodes_guard.insert(packet.barcode.clone());
        }
    }
    Ok(())
}

// Ses çalma fonksiyonu
fn play_sound(sound_data: &'static [u8]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    // Ses çıkış akışını ve sink'i oluştur
    let (_stream, stream_handle) = OutputStream::try_default()?;
    let sink = Sink::try_new(&stream_handle)?;

    // Gömülü ses verisinden bir okuyucu oluştur
    let cursor = Cursor::new(sound_data);

    // Ses verisini decode et
    let source = Decoder::new(cursor)?;

    // Sesi sink'e ekle ve oynat
    sink.append(source);

    // Sesin bitmesini bekle
    sink.sleep_until_end();

    Ok(())
}

async fn paste_latest_barcode(barcode_str: String) -> Result<(), Box<dyn std::error::Error>> {
    if barcode_str.is_empty() {
        println!("No printed barcode string.");
        return Ok(());
    }

    let mut printed_barcode: std::sync::MutexGuard<'_, HashSet<String>> =
        PRINTED_BARCODES.lock().unwrap();
    if printed_barcode.contains(&barcode_str) {
        println!("Barcode: {} Already printed!", barcode_str);
        spawn(async {
            if let Err(e) = play_sound(*ERROR_BARCODE_SOUND) {
                eprintln!("Error playing error barcode sound: {}", e);
            }
        });
        return Ok(());
    }

    // Set clipboard content
    let mut ctx: ClipboardContext = ClipboardProvider::new()?;
    ctx.set_contents(barcode_str.clone())?;

    // Simulate Ctrl+V to paste the content
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(100)); // Wait briefly to ensure the target application is ready

        #[cfg(target_os = "macos")]
        let modifier = Key::MetaLeft;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::ControlLeft;

        simulate(&EventType::KeyPress(modifier)).unwrap();
        simulate(&EventType::KeyPress(Key::KeyV)).unwrap();
        simulate(&EventType::KeyRelease(Key::KeyV)).unwrap();
        simulate(&EventType::KeyRelease(modifier)).unwrap();
        thread::sleep(Duration::from_millis(50));

        simulate(&EventType::KeyPress(Key::Return)).unwrap();
        simulate(&EventType::KeyRelease(Key::Return)).unwrap();
    });

    println!("Printed barcode: {}", barcode_str);
    printed_barcode.insert(barcode_str.clone());

    // Barkod başarıyla yapıştırıldığında başarı sesi çal
    // Ses çalma işini yeni bir spawn blok içinde çalıştırmak, ana luppun tıkanmamasını sağlar.
    spawn(async {
        if let Err(e) = play_sound(*SUCCESS_SOUND) {
            eprintln!("Error playing success sound: {}", e);
        }
    });

    Ok(())
}

use serialport::SerialPort;
async fn listen_com_port() -> Result<(), Box<dyn std::error::Error>> {
    let port_name = "COM5";
    let baud_rate = 9600;
    let timeout_ms = 100;
    let retry_delay_secs = 5;

    loop {
        println!(
            "Attempting to open serial port: {} with baud rate {}",
            port_name, baud_rate
        );

        let mut port: Box<dyn SerialPort> = match serialport::new(port_name, baud_rate)
            .timeout(Duration::from_millis(timeout_ms))
            .open()
        {
            Ok(p) => {
                println!("Serial port {} successfully opened.", port_name);
                p
            }
            Err(e) => {
                eprintln!("Failed to open serial port {}: {}", port_name, e);
                eprintln!("Retrying in {} seconds...", retry_delay_secs);
                time::sleep(Duration::from_secs(retry_delay_secs)).await;
                continue;
            }
        };

        let mut buffer: Vec<u8> = vec![0; 128];
        let mut received_data = String::new();

        loop {
            match port.read(buffer.as_mut_slice()) {
                Ok(bytes_read) => {
                    if bytes_read > 0 {
                        let received_str = String::from_utf8_lossy(&buffer[..bytes_read]);
                        received_data.push_str(&received_str);

                        if received_data.contains('\n') || received_data.len() >= 18 {
                            let parts: Vec<&str> = received_data.split('\n').collect();
                            for part in parts {
                                let trimmed_part = part.trim();
                                if trimmed_part.len() == 18 {
                                    let incoming_barcode = trimmed_part.to_string();
                                    println!(
                                        "Received barcode from COM port: {}",
                                        incoming_barcode
                                    );

                                    let server_valid_barcodes =
                                        SERVER_VALID_BARCODES.lock().unwrap();
                                    if server_valid_barcodes.contains(&incoming_barcode) {
                                        println!(
                                            "Matching barcode found! Preparing to paste: {}",
                                            incoming_barcode
                                        );
                                        if let Err(e) =
                                            paste_latest_barcode(incoming_barcode.clone()).await
                                        {
                                            eprintln!(
                                                "Error pasting barcode from COM port match: {}",
                                                e
                                            );
                                            spawn(async {
                                                if let Err(e) = play_sound(*ERROR_SOUND) {
                                                    eprintln!("Error playing error sound: {}", e);
                                                }
                                            });
                                        }
                                    } else {
                                        println!("Received barcode from COM port does not match any server barcode: {}", incoming_barcode);
                                        spawn(async {
                                            if let Err(e) = play_sound(*ERROR_TEST_SOUND) {
                                                eprintln!("Error playing error test sound: {}", e);
                                            }
                                        });
                                    }
                                } else if !trimmed_part.is_empty() {
                                    println!("Received partial or invalid data from COM port: '{}' (length: {})", trimmed_part, trimmed_part.len());
                                }
                            }
                            received_data.clear();
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                    // Zaman aşımı, veri yok ama bağlantı hala açık olabilir
                }
                Err(e) => {
                    eprintln!("Error reading from serial port: {}", e);
                    break;
                }
            }
            time::sleep(Duration::from_millis(50)).await;
        }
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    println!("Coskun ERGAN 2025");
    println!("Compiled time: {}", COMPILED_AT);
    println!("Rust Version: {}", RUST_COMPILER_VERSION);
    {
        println!("Server started at: {}", *STARTUP_TIME);
    }
    println!("Server Version : 1.1");

    spawn(async {
        loop {
            if let Err(e) = collect_latest_barcode().await {
                println!("Error collecting barcode: {}", e);
            }
            time::sleep(Duration::from_secs(5)).await;
        }
    });

    spawn(async {
        if let Err(e) = listen_com_port().await {
            eprintln!("Error listening on COM port: {}", e);
        }
    });

    HttpServer::new(|| App::new())
        .bind("127.0.0.1:8085")?
        .run()
        .await
}
