//! Benchmarks demonstrating `insert_with` benefits for partial initialization.
#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::alloc::Layout;
use std::hint::black_box;
use std::mem::MaybeUninit;
use std::time::Instant;

use alloc_tracker::Allocator;
use criterion::{Criterion, criterion_group, criterion_main};
use opaque_pool::OpaquePool;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

/// A large buffer that benefits from partial initialization.
/// Only metadata needs to be initialized initially; data buffer can remain uninitialized.
#[allow(dead_code, reason = "benchmark struct fields used during construction")]
struct PartialBuffer {
    capacity: usize,
    len: usize,
    /// Large data buffer that we want to avoid initializing unnecessarily.
    data: [u8; 4096],
    checksum: MaybeUninit<u32>,
}

impl PartialBuffer {
    /// Traditional constructor that zero-initializes the entire 4KB buffer.
    fn new_traditional(capacity: usize) -> Self {
        Self {
            capacity,
            len: 0,
            data: [0; 4096], // Expensive: writes 4KB of zeros
            checksum: MaybeUninit::uninit(),
        }
    }

    /// Partial initialization - only initializes metadata fields.
    fn new_in_place(uninit: &mut MaybeUninit<Self>, capacity: usize) {
        let ptr = uninit.as_mut_ptr();

        // SAFETY: We have exclusive access to this uninitialized memory.
        let capacity_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).capacity) };
        // SAFETY: We have exclusive access to this uninitialized memory.
        let len_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).len) };

        // Only initialize the fields we need right now.
        // SAFETY: We're writing to our own struct's capacity field.
        unsafe {
            capacity_ptr.write(capacity);
        }
        // SAFETY: We're writing to our own struct's len field.
        unsafe {
            len_ptr.write(0);
        }
        // Note: data and checksum remain uninitialized, saving 4KB+ of writes
    }

    /// Access method to prevent dead code warnings.
    fn capacity(&self) -> usize {
        self.capacity
    }
}

/// Cache entry that starts with only key initialized.
#[allow(
    dead_code,
    reason = "benchmark struct to demonstrate partial initialization"
)]
struct CacheEntry {
    key: String,
    value: MaybeUninit<Vec<u8>>,
    computed_at: MaybeUninit<Instant>,
}

impl CacheEntry {
    /// Traditional constructor that initializes all fields.
    fn new_traditional(key: String) -> Self {
        Self {
            key,
            value: MaybeUninit::new(Vec::new()), // Allocates empty Vec
            computed_at: MaybeUninit::new(Instant::now()), // Gets current time
        }
    }

    /// Partial initialization - only initializes the key.
    fn new_in_place(uninit: &mut MaybeUninit<Self>, key: String) {
        let ptr = uninit.as_mut_ptr();

        // SAFETY: We have exclusive access to this uninitialized memory.
        let key_ptr = unsafe { std::ptr::addr_of_mut!((*ptr).key) };

        // SAFETY: We're writing to our own struct's key field.
        unsafe {
            key_ptr.write(key);
        }
        // Note: value and computed_at remain uninitialized
    }

    /// Access method to prevent dead code warnings.
    fn key(&self) -> &str {
        &self.key
    }
}

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("partial_initialization");

    // Benchmark: Traditional construction with full initialization of large buffer.
    let allocs_op = allocs.operation("traditional_full_init");
    group.bench_function("traditional_full_init", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<PartialBuffer>();
            let mut pool = OpaquePool::builder().layout(layout).build();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for i in 0..iters {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "benchmark uses small iteration counts"
                )]
                // Traditional approach: initializes entire 4KB buffer with zeros.
                let item = black_box(PartialBuffer::new_traditional(i as usize));
                // SAFETY: PartialBuffer matches the layout used to create the pool.
                let handle = black_box(unsafe { pool.insert(item) });
                black_box(handle.capacity());
                pool.remove_mut(handle);
            }

            start.elapsed()
        });
    });

    // Benchmark: Partial initialization using insert_with.
    let allocs_op = allocs.operation("partial_init_insert_with");
    group.bench_function("partial_init_insert_with", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<PartialBuffer>();
            let mut pool = OpaquePool::builder().layout(layout).build();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for i in 0..iters {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "benchmark uses small iteration counts"
                )]
                // Optimized approach: only initializes metadata, leaving 4KB data buffer untouched.
                // SAFETY: PartialBuffer matches the layout used to create the pool.
                let handle = black_box(unsafe {
                    pool.insert_with(|uninit| {
                        PartialBuffer::new_in_place(uninit, black_box(i as usize));
                    })
                });
                black_box(handle.capacity());
                pool.remove_mut(handle);
            }

            start.elapsed()
        });
    });

    group.finish();

    let mut group = c.benchmark_group("cache_entry_initialization");

    // Benchmark: Traditional cache entry with eager initialization.
    let allocs_op = allocs.operation("cache_traditional");
    group.bench_function("cache_traditional", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<CacheEntry>();
            let mut pool = OpaquePool::builder().layout(layout).build();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for i in 0..iters {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "benchmark uses small iteration counts"
                )]
                // Traditional: initializes value Vec and timestamp immediately.
                let key = format!("key_{}", i as usize);
                let item = black_box(CacheEntry::new_traditional(key));
                // SAFETY: CacheEntry matches the layout used to create the pool.
                let handle = black_box(unsafe { pool.insert(item) });
                black_box(handle.key());
                pool.remove_mut(handle);
            }

            start.elapsed()
        });
    });

    // Benchmark: Partial cache entry initialization.
    let allocs_op = allocs.operation("cache_partial_init");
    group.bench_function("cache_partial_init", |b| {
        b.iter_custom(|iters| {
            let layout = Layout::new::<CacheEntry>();
            let mut pool = OpaquePool::builder().layout(layout).build();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for i in 0..iters {
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "benchmark uses small iteration counts"
                )]
                // Optimized: only initializes key, defers value and timestamp.
                let key = format!("key_{}", i as usize);
                // SAFETY: CacheEntry matches the layout used to create the pool.
                let handle = black_box(unsafe {
                    pool.insert_with(|uninit| {
                        CacheEntry::new_in_place(uninit, black_box(key));
                    })
                });
                black_box(handle.key());
                pool.remove_mut(handle);
            }

            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
