use chrono::{Utc, Duration};
use std::process::Command;

fn main() {    
    let now_utc = Utc::now();
    let now_istanbul = now_utc + Duration::hours(3); // UTC'ye 3 saat ekle
    let compile_time = now_istanbul.to_rfc3339(); // ISO 8601 formatında

    println!("cargo:rustc-env=COMPILED_AT={}", compile_time);

    let rustc_output = Command::new("rustc")
        .arg("--version")
        .output()
        .expect("Failed to execute rustc --version");

    let rust_version = String::from_utf8_lossy(&rustc_output.stdout);
    let rust_version = rust_version.trim();
    println!("cargo:rustc-env=RUST_COMPILER_VERSION={}", rust_version);

    println!("cargo:rerun-if-changed=build.rs");
}