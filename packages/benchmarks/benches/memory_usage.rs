//! We demonstrate how to track memory usage (bytes per itration) in Criterion benchmarks.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::System;
use std::collections::HashMap;
use std::fmt;
use std::hint::black_box;
use std::sync::atomic;
use std::sync::atomic::AtomicU64;
use std::time::Instant;

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use tracking_allocator::{AllocationRegistry, AllocationTracker, Allocator};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<System> = Allocator::system();

fn entrypoint(c: &mut Criterion) {
    AllocationRegistry::set_global_tracker(MemoryTracker).unwrap();
    AllocationRegistry::enable_tracking();

    let mut memory_results = MemoryUsageResults::new();

    let mut group = c.benchmark_group("memory_usage");

    group.bench_function("string_formatting", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter(|| {
            let _contributor = average_memory_delta.contribute();

            let part1 = black_box("Hello, ");
            let part2 = black_box("world!");
            let s = format!("{part1}{part2}!");
            black_box(s);
        });

        memory_results.add(
            "string_formatting".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("string_formatting_batched", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter_batched_ref(
            || (),
            |()| {
                let _contributor = average_memory_delta.contribute();

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                black_box(s);
            },
            BatchSize::SmallInput,
        );

        memory_results.add(
            "string_formatting_batched".to_string(),
            average_memory_delta.average(),
        );
    });

    group.bench_function("string_formatting_custom", |b| {
        let mut average_memory_delta = AverageMemoryDelta::new();

        b.iter_custom(|iters| {
            let start = Instant::now();

            for _ in 0..iters {
                let _contributor = average_memory_delta.contribute();

                let part1 = black_box("Hello, ");
                let part2 = black_box("world!");
                let s = format!("{part1}{part2}!");
                drop(black_box(s));
            }

            start.elapsed()
        });

        memory_results.add(
            "string_formatting_custom".to_string(),
            average_memory_delta.average(),
        );
    });

    group.finish();

    AllocationRegistry::disable_tracking();

    println!("{memory_results}");
}

// The tracking allocator works with static data all over the place, so this is how it be.
static TRACKER_BYTES_ALLOCATED: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
struct MemoryTracker;

impl AllocationTracker for MemoryTracker {
    fn allocated(
        &self,
        _addr: usize,
        object_size: usize,
        _wrapped_size: usize,
        _group_id: tracking_allocator::AllocationGroupId,
    ) {
        TRACKER_BYTES_ALLOCATED.fetch_add(object_size as u64, atomic::Ordering::Relaxed);
    }

    fn deallocated(
        &self,
        _addr: usize,
        _object_size: usize,
        _wrapped_size: usize,
        _source_group_id: tracking_allocator::AllocationGroupId,
        _current_group_id: tracking_allocator::AllocationGroupId,
    ) {
    }
}

#[derive(Debug)]
struct MemoryDeltaTracker {
    initial_bytes_allocated: u64,
}

impl MemoryDeltaTracker {
    fn new() -> Self {
        let initial_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        Self {
            initial_bytes_allocated,
        }
    }

    fn to_delta(&self) -> u64 {
        let final_bytes_allocated = TRACKER_BYTES_ALLOCATED.load(atomic::Ordering::Relaxed);

        final_bytes_allocated
            .checked_sub(self.initial_bytes_allocated)
            .expect("total bytes allocated could not possibly decrease")
    }
}

#[derive(Debug)]
struct AverageMemoryDelta {
    total_bytes_allocated: u64,
    iterations: u64,
}

impl AverageMemoryDelta {
    fn new() -> Self {
        Self {
            total_bytes_allocated: 0,
            iterations: 0,
        }
    }

    fn add(&mut self, delta: u64) {
        // Never going to overflow u64, so no point doing slower checked arithmetic here.
        self.total_bytes_allocated = self.total_bytes_allocated.wrapping_add(delta);
        self.iterations = self.iterations.wrapping_add(1);
    }

    fn contribute(&mut self) -> AverageMemoryDeltaContributor<'_> {
        AverageMemoryDeltaContributor::new(self)
    }

    #[expect(clippy::integer_division, reason = "we accept loss of precision")]
    #[expect(
        clippy::arithmetic_side_effects,
        reason = "division by zero excluded via if-else"
    )]
    fn average(&self) -> u64 {
        if self.iterations == 0 {
            0
        } else {
            self.total_bytes_allocated / self.iterations
        }
    }
}

#[derive(Debug)]
struct AverageMemoryDeltaContributor<'a> {
    average_memory_delta: &'a mut AverageMemoryDelta,
    memory_delta_tracker: MemoryDeltaTracker,
}

impl<'a> AverageMemoryDeltaContributor<'a> {
    fn new(average_memory_delta: &'a mut AverageMemoryDelta) -> Self {
        let memory_delta_tracker = MemoryDeltaTracker::new();

        Self {
            average_memory_delta,
            memory_delta_tracker,
        }
    }
}

impl Drop for AverageMemoryDeltaContributor<'_> {
    fn drop(&mut self) {
        let delta = self.memory_delta_tracker.to_delta();
        self.average_memory_delta.add(delta);
    }
}

#[derive(Debug)]
struct MemoryUsageResults {
    average_allocated_per_benchmark_iteration: HashMap<String, u64>,
}

impl MemoryUsageResults {
    fn new() -> Self {
        Self {
            average_allocated_per_benchmark_iteration: HashMap::new(),
        }
    }

    fn add(&mut self, name: String, average: u64) {
        self.average_allocated_per_benchmark_iteration
            .insert(name, average);
    }
}

impl fmt::Display for MemoryUsageResults {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Memory allocated:")?;

        for (name, average) in &self.average_allocated_per_benchmark_iteration {
            writeln!(f, "{name}: {average} bytes per iteration")?;
        }
        Ok(())
    }
}
