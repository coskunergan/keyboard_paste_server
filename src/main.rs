use actix_web::rt::{spawn, time};
use actix_web::{App, HttpServer};
use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use clipboard::{ClipboardContext, ClipboardProvider};
use hashbrown::HashSet;
use rdev::{simulate, EventType, Key};
use reqwest::Error as ReqwestError;
use serde::Deserialize;
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
    static ref LATEST_BARCODE: Mutex<Option<String>> = Mutex::new(None);
    static ref PASTED_BARCODES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
    static ref STARTUP_TIME:  /*Mutex<*/DateTime<FixedOffset>/*>*/ = {
        let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +03:00 Istanbul
        /*Mutex::new(*/Utc::now().with_timezone(&timezone_offset)/*)*/
    };
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
    let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap();

    let mut pasted_barcodes = PASTED_BARCODES.lock().unwrap();

    // Get the last packet with status "OK", valid barcode, and timestamp after startup
    let latest_valid_barcode = packets.into_iter().rev().find(|packet| {
        // Validate status and barcode length
        if packet.status != "OK"
        || packet.barcode.len() != 18  // burası açılacak!!!
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
    let mut barcode = LATEST_BARCODE.lock().unwrap();
    match latest_valid_barcode {
        Some(packet) => {
            *barcode = Some(packet.barcode.clone());
            pasted_barcodes.insert(packet.barcode.clone());
            println!(
                "Stored latest barcode: {} (timestamp: {})",
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
    println!("Server Version : 1.0");

    // Start a background task to collect the latest barcode every 5 seconds
    spawn(async {
        loop {
            if let Err(e) = collect_latest_barcode().await {
                println!("Error collecting barcode: {}", e);
            }
            time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Start a separate task to paste the latest barcode every 3100 seconds
    spawn(async {
        loop {
            time::sleep(Duration::from_secs(10)).await;
            if let Err(e) = paste_latest_barcode().await {
                println!("Error pasting barcode: {}", e);
            }
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
