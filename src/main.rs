use actix_web::rt::{spawn, time};
use actix_web::{App, HttpServer};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
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
    static ref LATEST_BARCODE: Mutex<Option<String>> = Mutex::new(None);
    static ref PASTED_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    static ref STARTUP_TIME:  /*Mutex<*/DateTime<FixedOffset>/*>*/ = {
        let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +03:00 Istanbul
        /*Mutex::new(*/Utc::now().with_timezone(&timezone_offset)/*)*/
    };
    static ref SERVER_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new()); // Sunucudan gelen barkodları tutmak için yeni HashSet
}

#[derive(Deserialize)]
struct Packet {
    barcode: String,
    status: String,
    timestamp: String,
    // Other fields are ignored for deserialization
}

async fn fetch_packets() -> Result<Vec<Packet>, ReqwestError> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://172.22.5.196:8080/packets") // burası açılacak!!!
        //.get("http://localhost:8080/packets")
        .send()
        .await?
        .json::<Vec<Packet>>()
        .await?;
    Ok(response)
}

async fn collect_latest_barcode() -> Result<(), Box<dyn std::error::Error>> {
    // Fetch packets from the remote server
    let packets = fetch_packets().await?;

    // Get the server startup time
    let startup_time = *STARTUP_TIME/*.lock().unwrap()*/;
    let timezone_offset = FixedOffset::east_opt(0 * 3600).unwrap(); // son 3 saati yazdırabilir.

    let pasted_barcodes = PASTED_BARCODES.lock().unwrap();
    let mut server_barcodes = SERVER_BARCODES.lock().unwrap(); // Server barkodlarını güncellemek için

    // Get the last packet with status "OK", valid barcode, and timestamp after startup
    let latest_valid_barcode = packets.into_iter().rev().find(|packet| {
        // Validate status and barcode length
        if packet.status != "OK" || packet.barcode.len() != 18
        {
            return false;
        }

        if pasted_barcodes.contains(&packet.barcode) {
            return false;
        }

        // Parse timestamp as NaiveDateTime
        let naive_timestamp =
            match NaiveDateTime::parse_from_str(&packet.timestamp, "%Y-%m-%d %H:%M:%S") {
                Ok(naive) => naive,
                Err(_) => {
                    println!("Invalid timestamp format: {}", packet.timestamp);
                    return false;
                }
            };

        // Combine NaiveDateTime with FixedOffset to get DateTime<FixedOffset>
        let timestamp_result = timezone_offset.from_local_datetime(&naive_timestamp);

        match timestamp_result {
            chrono::LocalResult::Single(ts) => ts > startup_time,
            chrono::LocalResult::Ambiguous(ts1, ts2) => ts1 > startup_time || ts2 > startup_time,
            chrono::LocalResult::None => {
                println!(
                    "Error converting naive timestamp to fixed offset DateTime (None case): {}",
                    packet.timestamp
                );
                false
            }
        }
    });

    // Update the global latest barcode
    
    match latest_valid_barcode {
        Some(packet) => {
            // Add all valid barcodes from the server to SERVER_BARCODES for later comparison with COM port data
            server_barcodes.insert(packet.barcode.clone());
            println!(
                "Stored latest barcode from server: {} (timestamp: {})",
                packet.barcode, packet.timestamp
            );
        }
        None => {
            //println!("No valid barcode with status 'OK' and timestamp after startup found");
        }
    }

    Ok(())
}

async fn paste_latest_barcode() -> Result<(), Box<dyn std::error::Error>> {
    // Get the latest barcode
    let mut barcode = LATEST_BARCODE.lock().unwrap();
    if barcode.is_none() {
        //println!("No barcode to paste");
        return Ok(());
    }

    // Get the barcode string
    let text = barcode.as_ref().unwrap().clone();

    // Set clipboard content
    let mut ctx: ClipboardContext = ClipboardProvider::new()?;
    ctx.set_contents(text.clone())?;

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

    *barcode = None;
    println!("Printed barcode: {}", text);
    Ok(())
}

// Yeni eklenen fonksiyon: COM portu dinler ve barkodları işler
async fn listen_com_port() -> Result<(), Box<dyn std::error::Error>> {
    let port_name = "COM3"; // Burayı kendi COM port numaranızla değiştirin (örn: "COM1", "COM2" vb.)
    let baud_rate = 9600; // Barkod okuyucunuzun baud rate'ini buraya girin

    println!(
        "Attempting to open serial port: {} with baud rate {}",
        port_name, baud_rate
    );

    let mut port = serialport::new(port_name, baud_rate)
        .timeout(Duration::from_millis(100)) // Okuma zaman aşımı
        .open()?;

    let mut buffer: Vec<u8> = vec![0; 128]; // Okuma tamponu
    let mut received_data = String::new();

    loop {
        match port.read(buffer.as_mut_slice()) {
            Ok(bytes_read) => {
                if bytes_read > 0 {
                    let received_str = String::from_utf8_lossy(&buffer[..bytes_read]);
                    received_data.push_str(&received_str);

                    // Yeni satır karakteri veya yeterli uzunlukta bir barkod algılarsak işleyelim
                    if received_data.contains('\n') || received_data.len() >= 18 {
                        // Yeni satır karakterine göre bölelim
                        let parts: Vec<&str> = received_data.split('\n').collect();
                        for part in parts {
                            let trimmed_part = part.trim();
                            if trimmed_part.len() == 18 {
                                let incoming_barcode = trimmed_part.to_string();
                                println!("Received barcode from COM port: {}", incoming_barcode);

                                let server_barcodes = SERVER_BARCODES.lock().unwrap();
                                if server_barcodes.contains(&incoming_barcode) {
                                    println!(
                                        "Matching barcode found! Preparing to paste: {}",
                                        incoming_barcode
                                    );                                                                   
                                    let mut latest_barcode_guard = LATEST_BARCODE.lock().unwrap();
                                    *latest_barcode_guard = Some(incoming_barcode.clone()); // Eşleşen barkodu ayarla
                                    drop(latest_barcode_guard); // Mutex kilidini bırak

                                    // paste_latest_barcode'u doğrudan çağır
                                    if let Err(e) = paste_latest_barcode().await {
                                        eprintln!(
                                            "Error pasting barcode from COM port match: {}",
                                            e
                                        );
                                    }
                                } else {
                                    println!("Received barcode from COM port does not match any server barcode: {}", incoming_barcode);
                                }
                            } else if !trimmed_part.is_empty() {
                                // 18 karakter olmayan ama boş olmayan veriyi de yazdıralım
                                //println!("Received partial or invalid data from COM port: '{}' (length: {})", trimmed_part, trimmed_part.len());
                            }
                        }
                        received_data.clear(); // İşlenen veriyi temizle
                    }
                }
            }
            Err(ref e) if e.kind() == io::ErrorKind::TimedOut => {
                // Zaman aşımı, veri yok, döngü devam etsin
            }
            Err(e) => {
                eprintln!("Error reading from serial port: {}", e);
                // Hata durumunda kısa bir süre bekleyip tekrar denemek faydalı olabilir
                time::sleep(Duration::from_secs(1)).await;
            }
        }
        time::sleep(Duration::from_millis(50)).await; // İşlemciyi yormamak için kısa bir bekleme
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
            *STARTUP_TIME /*.lock().unwrap()*/
        );
    }
    println!("Server Version : 1.1");

    // Start a background task to collect the latest barcode every 5 seconds
    // Bu görev artık aynı zamanda SERVER_BARCODES'i de güncelleyecek
    spawn(async {
        loop {
            if let Err(e) = collect_latest_barcode().await {
                println!("Error collecting barcode: {}", e);
            }
            time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Start a separate task to paste the latest barcode every 3100 seconds
    // Bu görev sadece sunucudan gelen ve eşleşen barkodu yapıştırmak için kalacak
    spawn(async {
        loop {
            time::sleep(Duration::from_secs(10)).await; // Bu kısım hala gerekli mi kontrol edilmeli, eğer paste_latest_barcode() sadece COM portundan tetiklenecekse bu kısım kaldırılabilir.
                                                        // COM portundan bir eşleşme geldiğinde LATEST_BARCODE dolduğu için, burası da onu yapıştırabilir.
            if let Err(e) = paste_latest_barcode().await {
                println!("Error pasting barcode from timed task: {}", e);
            }
        }
    });

    // Yeni: COM port dinleme görevini başlat
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
