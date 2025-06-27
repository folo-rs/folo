//! Allocates a region-cached variable with 1 GB of data and accesses it from every thread.
//!
//! You can observe memory usage to prove that the data is not being copied an unexpected
//! number of times (one copy per memory region is expected, plus one global primary copy).

use std::hint::black_box;
use std::thread;
use std::time::Duration;

use many_cpus::ProcessorSet;
use region_cached::{RegionCachedExt, region_cached};

region_cached! {
    static DATA: Vec<u8> = vec![50; 1024 * 1024 * 1024];
}

fn main() {
    // Exit early if running in a testing environment
    if std::env::var("IS_TESTING").is_ok() {
        println!("Running in testing mode - exiting immediately to prevent infinite operation");
        return;
    }

    let processor_set = ProcessorSet::default();

    processor_set
        .spawn_threads(|_| DATA.with_cached(|data| _ = black_box(data.len())))
        .into_iter()
        .for_each(|x| x.join().unwrap());

    println!(
        "All {} threads have accessed the region-cached data. Terminating in 60 seconds.",
        processor_set.len()
    );

    thread::sleep(Duration::from_secs(60));
}
