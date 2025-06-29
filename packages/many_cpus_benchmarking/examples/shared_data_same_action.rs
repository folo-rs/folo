//! Example demonstrating multiple threads operating on shared data structure, performing the same action.
//!
//! This scenario shows two workers that both read from a shared `HashMap`. Both workers perform the
//! same type of operation (reading), but they may be accessing the data from different memory
//! regions depending on the work distribution mode selected.

#![allow(missing_docs, reason = "No need for API documentation in example code")]

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::{Arc, RwLock};

use criterion::{Criterion, criterion_group, criterion_main};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

fn entrypoint(c: &mut Criterion) {
    // Workers require collaboration (one initializes, the other waits), so we cannot use "self" 
    // modes where workers operate independently. The "self" modes would cause deadlock since
    // the non-initializer worker would wait forever for initialization that never happens.
    execute_runs::<SharedHashMapRead<1024, 10>, 100>(
        c,
        WorkDistribution::all_with_unique_processors_without_self(),
    );
}

const _MAP_ENTRY_COUNT: usize = 1024;
const _REPEAT_COUNT: usize = 10;

/// Both workers read from the same shared `HashMap` data structure.
///
/// This demonstrates the "same action" scenario where multiple threads perform identical
/// operations on shared data. The performance differences between work distribution modes
/// will highlight the impact of memory locality - when workers are on different memory
/// regions, they may experience slower access to the shared data.
#[derive(Debug, Default)]
struct SharedHashMapRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    /// The shared `HashMap` that both workers will read from.
    map: Arc<RwLock<HashMap<u64, u64>>>,

    /// Only one worker needs to populate the map during preparation.
    is_initializer: bool,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for SharedHashMapRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        // Create a shared HashMap wrapped in Arc<RwLock<>> for safe concurrent access
        let map = Arc::new(RwLock::new(HashMap::with_capacity(MAP_ENTRY_COUNT)));

        // First worker will initialize the data
        let worker1 = Self {
            map: Arc::clone(&map),
            is_initializer: true,
        };

        // Second worker will only read the data
        let worker2 = Self {
            map,
            is_initializer: false,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        // Only the initializer worker populates the map
        if !self.is_initializer {
            return;
        }

        let mut map = self.map.write().unwrap();

        // Fill the map with some data
        for i in 0..MAP_ENTRY_COUNT {
            map.insert(i as u64, i.wrapping_mul(2) as u64);
        }
    }

    fn process(&mut self) {
        // Both workers perform the same action: reading from the map
        let map = self.map.read().unwrap();

        for _ in 0..REPEAT_COUNT {
            for key in 0..MAP_ENTRY_COUNT {
                // Read each entry and use black_box to prevent optimization
                black_box(map.get(&(key as u64)));
            }
        }
    }
}
