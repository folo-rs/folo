//! Allocates a region-local variable with 1 GB of data and accesses it from every thread.
//!
//! You can observe memory usage to prove that the data is not being copied an unexpected
//! number of times (one copy per memory region is expected).

use std::hint::black_box;
use std::thread;
use std::time::Duration;

use many_cpus::ProcessorSet;
use region_local::{RegionLocalExt, region_local};

region_local! {
    static DATA: Vec<u8> = vec![50; 1024 * 1024 * 1024];
}

fn main() {
    let processor_set = ProcessorSet::default();

    processor_set
        .spawn_threads(|_| DATA.with_local(|data| _ = black_box(data.len())))
        .into_iter()
        .for_each(|x| x.join().unwrap());

    println!(
        "All {} threads have accessed the region-local data. Terminating in 60 seconds.",
        processor_set.len()
    );

    thread::sleep(Duration::from_secs(60));
}
