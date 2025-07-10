use actix_web::rt::{spawn, time};
use actix_web::{App, HttpServer};
use chrono::{DateTime, FixedOffset, NaiveDateTime, Utc};
use clipboard::{ClipboardContext, ClipboardProvider};
use rdev::{simulate, EventType, Key};
use reqwest::Error as ReqwestError;
use serde::Deserialize;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;
use hashbrown::HashSet;

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
        //.get("http://172.22.5.196:8080/packets")
        .get("http://localhost:8080/packets")
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

    // Get the last packet with status "OK", valid barcode, and timestamp after startup
    let latest_valid_barcode = packets.into_iter().rev().find(|packet| {
        // Validate status and barcode length
        if packet.status != "OK"
        /*|| packet.barcode.len() != 18*/
        {
            return false;
        }

        // Parse timestamp as local time in +03:00 (already offset-corrected)
        let timestamp = NaiveDateTime::parse_from_str(&packet.timestamp, "%Y-%m-%d %H:%M:%S")
            .map(|naive| DateTime::<FixedOffset>::from_local(naive, timezone_offset));

        match timestamp {
            Ok(ts) => ts > startup_time,
            Err(_) => {
                println!("Invalid timestamp format: {}", packet.timestamp);
                false
            }
        }
    });

    // Update the global latest barcode
    let mut barcode = LATEST_BARCODE.lock().unwrap();
    match latest_valid_barcode {
        Some(packet) => {
            *barcode = Some(packet.barcode.clone());
            println!(
                "Stored latest barcode: {} (timestamp: {})",
                packet.barcode, packet.timestamp
            );
        }
        None => {
            println!("No valid barcode with status 'OK' and timestamp after startup found");
        }
    }

    Ok(())
}

async fn paste_latest_barcode() -> Result<(), Box<dyn std::error::Error>> {
    // Get the latest barcode
    let barcode = LATEST_BARCODE.lock().unwrap();
    if barcode.is_none() {
        println!("No barcode to paste");
        return Ok(());
    }

    //let timezone_offset = FixedOffset::east_opt(3 * 3600).unwrap(); // +03:00 Istanbul
    //let now = Utc::now().with_timezone(&timezone_offset);
    //*STARTUP_TIME.lock().unwrap() = now;

    // Get the barcode string
    let text = barcode.as_ref().unwrap().clone();

    let mut pasted_barcodes = PASTED_BARCODES.lock().unwrap();
    if pasted_barcodes.contains(&text) {
        println!("Barcode {} already pasted, skipping", text);
        return Ok(());
    }

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

    pasted_barcodes.insert(text.clone());
    println!("Pasted barcode: {}", text);
    Ok(())
}

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    // Log the startup time
    {
        println!("Server started at: {}", *STARTUP_TIME/*.lock().unwrap()*/);
    }

    // Start a background task to collect the latest barcode every 5 seconds
    spawn(async {
        loop {
            if let Err(e) = collect_latest_barcode().await {
                println!("Error collecting barcode: {}", e);
            }
            time::sleep(Duration::from_secs(5)).await;
        }
    });

    // Start a separate task to paste the latest barcode every 30 seconds
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

//0680002 2507018268
