//! Showcase basic use of the `region_local!` macro in a multithreaded web app.
//!
use axum::{Router, routing::get};
use many_cpus::HardwareInfo;
use region_local::{RegionLocalCopyExt, RegionLocalExt, region_local};
use std::time::{SystemTime, UNIX_EPOCH};

// A global variable whose value is unique in each memory region for fast local access.
// Writes to this variable are eventually consistent across all threads in the same memory region.
region_local!(static LAST_UPDATE: u128 = 0);

#[tokio::main]
async fn main() {
    let memory_region_count = HardwareInfo::max_memory_region_count();
    println!("the current system has {memory_region_count} memory regions");

    let app = Router::new()
        .route("/", get(read))
        .route("/update", get(update));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:1234").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Open `http://localhost:1234/` to read the current value.
async fn read() -> String {
    let last_update_timestamp = LAST_UPDATE.get_local();

    format!("Last update: {last_update_timestamp}")
}

/// Open `http://localhost:1234/update` to set a new value.
/// The new value is only visible to `read()` handlers that run in the same memory region.
async fn update() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    LAST_UPDATE.set_local(now);
    format!("Last update time set to: {}", now)
}
