use actix_web::rt::{spawn, time};
use actix_web::{App, HttpServer};
use chrono::{DateTime, FixedOffset, Utc};
use clipboard::{ClipboardContext, ClipboardProvider};
use hashbrown::HashSet;
use rdev::{simulate, EventType, Key};
use reqwest::Error as ReqwestError;
use serde::Deserialize;
use serialport::{self}; // serialport kütüphanesi eklendi
use std::io::{self, Read};
use std::sync::Mutex;
use std::thread;
use std::time::Duration; // io::Read trait'i eklendi

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
    static ref STARTUP_TIME:  /*Mutex<*/DateTime<FixedOffset>/*>*/ = {
        let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +03:00 Istanbul
        /*Mutex::new(*/Utc::now().with_timezone(&timezone_offset)/*)*/
    };
    static ref SERVER_VALID_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    static ref PRINTED_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
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
        //.get("http://localhost:8080/packets")
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

    //println!("Collected barcode count: {}", server_valid_barcodes_guard.len());

    Ok(())
}

async fn paste_latest_barcode(barcode_str: String) -> Result<(), Box<dyn std::error::Error>> {

    if barcode_str.is_empty() {
        println!("No printed barcode string.");
        return Ok(());
    }

    let mut printed_barcode: std::sync::MutexGuard<'_, HashSet<String>> = PRINTED_BARCODES.lock().unwrap();
    if printed_barcode.contains(&barcode_str) {
        println!("Barcode: {} Already printed!", barcode_str);
        return Ok(()); // bu kontrol ilk etapta kapatılabilir!
    }

    // Set clipboard content
    let mut ctx: ClipboardContext = ClipboardProvider::new()?;
    ctx.set_contents(barcode_str.clone())?;

    // Simulate Ctrl+V to paste the content
    thread::spawn(move || {
        // Wait briefly to ensure the target application is ready
        thread::sleep(Duration::from_millis(100));

        // Simulate Ctrl+V (or Cmd+V on macOS)
        #[cfg(target_os = "macos")]
        let modifier = Key::MetaLeft;
        #[cfg(not(target_os = "macos"))]
        let modifier = Key::ControlLeft;

        simulate(&EventType::KeyPress(modifier)).unwrap();
        simulate(&EventType::KeyPress(Key::KeyV)).unwrap();
        simulate(&EventType::KeyRelease(Key::KeyV)).unwrap();
        simulate(&EventType::KeyRelease(modifier)).unwrap();
        thread::sleep(Duration::from_millis(50));

        // Simulate pressing Return to complete the paste
        simulate(&EventType::KeyPress(Key::Return)).unwrap();
        simulate(&EventType::KeyRelease(Key::Return)).unwrap();
    });
    
    println!("Printed barcode: {}", barcode_str);
    
    printed_barcode.insert(barcode_str.clone());

    Ok(())
}

use serialport::SerialPort;
async fn listen_com_port() -> Result<(), Box<dyn std::error::Error>> {
    let port_name = "COM5";
    let baud_rate = 9600;
    let timeout_ms = 100;
    let retry_delay_secs = 5; // Yeniden deneme öncesi bekleme süresi

    // Dış döngü, bağlantı kesildiğinde yeniden bağlanmayı dener
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
                continue; // Yeniden bağlanma döngüsünü baştan başlat
            }
        };

        let mut buffer: Vec<u8> = vec![0; 128];
        let mut received_data = String::new();

        // İç döngü, port açık olduğu sürece verileri okur
        loop {
            match port.read(buffer.as_mut_slice()) {
                Ok(bytes_read) => {
                    if bytes_read > 0 {
                        let received_str = String::from_utf8_lossy(&buffer[..bytes_read]);
                        received_data.push_str(&received_str);

                        // Eğer satır sonu veya yeterli uzunlukta veri alındıysa işle
                        if received_data.contains('\n') || received_data.len() >= 18 {
                            let parts: Vec<&str> = received_data.split('\n').collect();
                            for part in parts {
                                let trimmed_part = part.trim();
                                if trimmed_part.len() == 18 {
                                    let incoming_barcode = trimmed_part.to_string();
                                    println!("Received barcode from COM port: {}", incoming_barcode);

                                    let server_valid_barcodes = SERVER_VALID_BARCODES.lock().unwrap();
                                    if server_valid_barcodes.contains(&incoming_barcode) {
                                        println!(
                                            "Matching barcode found! Preparing to paste: {}",
                                            incoming_barcode
                                        );

                                        // paste_string'in doğru argümanı alması için düzeltme
                                        if let Err(e) = paste_latest_barcode(incoming_barcode.clone()).await {
                                            eprintln!(
                                                "Error pasting barcode from COM port match: {}",
                                                e
                                            );
                                        }
                                    } else {
                                        println!("Received barcode from COM port does not match any server barcode: {}", incoming_barcode);
                                    }
                                } else if !trimmed_part.is_empty() {
                                    println!("Received partial or invalid data from COM port: '{}' (length: {})", trimmed_part, trimmed_part.len());
                                }
                            }
                            // İşlenen veriyi temizle, bir sonraki okuma için hazırla
                            received_data.clear();
                        }
                    }
                }
                Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                    // Zaman aşımı, veri yok ama bağlantı hala açık olabilir
                    // Devam et ve tekrar okumayı dene
                }
                Err(e) => {
                    // Diğer hatalar (örn. "Access is denied" veya bağlantı kesilmesi)
                    eprintln!("Error reading from serial port: {}", e);
                    // İç döngüden çık ve dış döngüden yeniden bağlanmayı dene
                    break;
                }
            }
            // Kısa bir bekleme süresi, CPU kullanımını azaltmak için
            time::sleep(Duration::from_millis(50)).await;
        }
        // Eğer iç döngüden çıkıldıysa (hata nedeniyle), yeniden bağlanmak için dış döngünün başına dön.
        // Dış döngü otomatik olarak 'continue' ile tekrar portu açmayı deneyecektir.
    }
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Log the startup time

    println!("Coskun ERGAN 2025");
    println!("Compiled time: {}", COMPILED_AT);
    println!("Rust Version: {}", RUST_COMPILER_VERSION);
    {
        println!(
            "Server started at: {}",
            *STARTUP_TIME
        );
    }
    println!("Server Version : 1.1");

    // Start a background task to collect the latest barcode every 5 seconds
    spawn(async {
        loop {
            if let Err(e) = collect_latest_barcode().await {
                println!("Error collecting barcode: {}", e);
            }
            time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Yeni: COM port listen task
    spawn(async {
        if let Err(e) = listen_com_port().await {
            eprintln!("Error listening on COM port: {}", e);
        }
    });

    // Start the HTTP server (though no endpoints are needed for this task)
    HttpServer::new(|| App::new())
        .bind("127.0.0.1:8085")?
        .run()
        .await
}

//sample barcode
//0680002 2507018268
