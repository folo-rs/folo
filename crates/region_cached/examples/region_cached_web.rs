use axum::{Router, routing::get};
use many_cpus::HardwareInfo;
use region_cached::{RegionCachedCopyExt, RegionCachedExt, region_cached};
use std::time::{SystemTime, UNIX_EPOCH};

// A global variable whose latest value is cached in each memory region for fast local read access.
// Writes to this variable are weakly consistent across all memory regions.
//
// Note: to keep the example simple, the value of this variable is of a trivial size and unlikely
// to actually benefit from region-local caching as it easily fits into local processor caches.
region_cached!(static LAST_UPDATE: u128 = 0);

#[tokio::main]
async fn main() {
    // The beneficial impact will arise only on systems with multiple memory regions.
    let memory_region_count = HardwareInfo::max_memory_region_count();
    println!("the current system has {memory_region_count} memory regions");

    let app = Router::new()
        .route("/", get(read))
        .route("/update", get(update));
    let listener = tokio::net::TcpListener::bind("0.0.0.0:1234").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

/// Open http://localhost:1234/ to read the current value.
async fn read() -> String {
    let last_update_timestamp = LAST_UPDATE.get_cached();

    format!("Last update: {last_update_timestamp}")
}

/// Open http://localhost:1234/update to set a new value.
async fn update() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    LAST_UPDATE.set_global(now);
    format!("Last update time set to: {}", now)
}
