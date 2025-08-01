//! Benchmarks for the `blind_pool` package with many different types.
//!
//! This benchmark suite tests how `BlindPool` performs when managing many internal pools
//! for different type layouts. It measures scenarios where pools already contain many
//! different types and we add items of existing vs. new layouts.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]

use std::hint::black_box;
use std::iter;
use std::time::Instant;

use alloc_tracker::Allocator;
use blind_pool::BlindPool;
use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

#[global_allocator]
static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();

// Macro to generate many types with different alignments and sizes
macro_rules! generate_types {
    ($($name:ident: $size:expr, $align:expr, $value:expr;)*) => {
        $(
            #[repr(C, align($align))]
            #[derive(Clone, Copy)]
            struct $name {
                data: [u8; $size],
            }

            impl $name {
                const fn with_value() -> Self {
                    Self { data: [$value; $size] }
                }
            }
        )*

        // Function to populate a pool with one item of each type
        fn populate_pool_with_all_types(pool: &mut BlindPool) {
            $(
                _ = pool.insert($name::with_value());
            )*
        }
    };
}

// Generate 100 different types with various alignments and sizes
generate_types! {
    Type001: 1, 1, 0x01;    Type002: 2, 1, 0x02;    Type003: 3, 1, 0x03;    Type004: 4, 1, 0x04;    Type005: 5, 1, 0x05;
    Type006: 6, 1, 0x06;    Type007: 7, 1, 0x07;    Type008: 8, 1, 0x08;    Type009: 9, 1, 0x09;    Type010: 10, 1, 0x0A;
    Type011: 1, 2, 0x0B;    Type012: 2, 2, 0x0C;    Type013: 4, 2, 0x0D;    Type014: 6, 2, 0x0E;    Type015: 8, 2, 0x0F;
    Type016: 10, 2, 0x10;   Type017: 12, 2, 0x11;   Type018: 14, 2, 0x12;   Type019: 16, 2, 0x13;   Type020: 18, 2, 0x14;
    Type021: 1, 4, 0x15;    Type022: 2, 4, 0x16;    Type023: 4, 4, 0x17;    Type024: 8, 4, 0x18;    Type025: 12, 4, 0x19;
    Type026: 16, 4, 0x1A;   Type027: 20, 4, 0x1B;   Type028: 24, 4, 0x1C;   Type029: 28, 4, 0x1D;   Type030: 32, 4, 0x1E;
    Type031: 1, 8, 0x1F;    Type032: 2, 8, 0x20;    Type033: 4, 8, 0x21;    Type034: 8, 8, 0x22;    Type035: 16, 8, 0x23;
    Type036: 24, 8, 0x24;   Type037: 32, 8, 0x25;   Type038: 40, 8, 0x26;   Type039: 48, 8, 0x27;   Type040: 56, 8, 0x28;
    Type041: 1, 16, 0x29;   Type042: 2, 16, 0x2A;   Type043: 4, 16, 0x2B;   Type044: 8, 16, 0x2C;   Type045: 16, 16, 0x2D;
    Type046: 32, 16, 0x2E;  Type047: 48, 16, 0x2F;  Type048: 64, 16, 0x30;  Type049: 80, 16, 0x31;  Type050: 96, 16, 0x32;
    Type051: 13, 1, 0x33;   Type052: 17, 1, 0x34;   Type053: 19, 1, 0x35;   Type054: 23, 1, 0x36;   Type055: 29, 1, 0x37;
    Type056: 31, 1, 0x38;   Type057: 37, 1, 0x39;   Type058: 41, 1, 0x3A;   Type059: 43, 1, 0x3B;   Type060: 47, 1, 0x3C;
    Type061: 15, 2, 0x3D;   Type062: 21, 2, 0x3E;   Type063: 25, 2, 0x3F;   Type064: 27, 2, 0x40;   Type065: 33, 2, 0x41;
    Type066: 35, 2, 0x42;   Type067: 39, 2, 0x43;   Type068: 45, 2, 0x44;   Type069: 49, 2, 0x45;   Type070: 51, 2, 0x46;
    Type071: 36, 4, 0x47;   Type072: 44, 4, 0x48;   Type073: 52, 4, 0x49;   Type074: 60, 4, 0x4A;   Type075: 68, 4, 0x4B;
    Type076: 76, 4, 0x4C;   Type077: 84, 4, 0x4D;   Type078: 92, 4, 0x4E;   Type079: 100, 4, 0x4F;  Type080: 108, 4, 0x50;
    Type081: 64, 8, 0x51;   Type082: 72, 8, 0x52;   Type083: 80, 8, 0x53;   Type084: 88, 8, 0x54;   Type085: 104, 8, 0x55;
    Type086: 112, 8, 0x56;  Type087: 120, 8, 0x57;  Type088: 128, 8, 0x58;  Type089: 136, 8, 0x59;  Type090: 144, 8, 0x5A;
    Type091: 112, 16, 0x5B; Type092: 128, 16, 0x5C; Type093: 144, 16, 0x5D; Type094: 160, 16, 0x5E; Type095: 176, 16, 0x5F;
    Type096: 192, 16, 0x60; Type097: 208, 16, 0x61; Type098: 224, 16, 0x62; Type099: 240, 16, 0x63; Type100: 256, 16, 0x64;
}

fn entrypoint(c: &mut Criterion) {
    let allocs = alloc_tracker::Session::new();

    let mut group = c.benchmark_group("blind_many_types");

    // First, test inserting an item of an existing layout
    let allocs_op = allocs.operation("insert_existing_layout");
    group.bench_function("insert_existing_layout", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(BlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-populate each pool with one item of each of the 100 types.
            for pool in &mut pools {
                populate_pool_with_all_types(pool);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            // Insert another item of Type001, which already exists in the pool.
            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(Type001::with_value())));
            }

            start.elapsed()
        });
    });

    // Test inserting an item of a completely new layout
    let allocs_op = allocs.operation("insert_new_layout");
    group.bench_function("insert_new_layout", |b| {
        b.iter_custom(|iters| {
            // Define a new type that doesn't exist in our 100 types.
            #[repr(C, align(32))]
            #[derive(Clone, Copy)]
            struct NewType {
                data: [u8; 77], // Unusual size not in our type list
            }

            impl NewType {
                const fn with_value() -> Self {
                    Self { data: [0xFF; 77] }
                }
            }

            let mut pools = iter::repeat_with(BlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-populate each pool with one item of each of the 100 types.
            for pool in &mut pools {
                populate_pool_with_all_types(pool);
                // Verify our belief that NewType doesn't exist in the pool yet.
                assert_eq!(pool.capacity_of::<NewType>(), 0);
            }

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            // Insert an item of NewType, which doesn't exist in the pool yet.
            for pool in &mut pools {
                _ = black_box(pool.insert(black_box(NewType::with_value())));
            }

            start.elapsed()
        });
    });

    // Test reading from a pool with many types
    let allocs_op = allocs.operation("read_from_many_types");
    group.bench_function("read_from_many_types", |b| {
        b.iter_custom(|iters| {
            let mut pool = BlindPool::new();

            // Pre-populate pool with one item of each of the 100 types.
            populate_pool_with_all_types(&mut pool);

            // Get a handle to the first type we can read from.
            let pooled = pool.insert(Type050::with_value());

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                // SAFETY: The pointer is valid and the memory contains the value we just inserted.
                _ = black_box(unsafe { pooled.ptr().read() });
            }

            start.elapsed()
        });
    });

    // Test len() operation on a pool with many types
    let allocs_op = allocs.operation("len_many_types");
    group.bench_function("len_many_types", |b| {
        b.iter_custom(|iters| {
            let mut pool = BlindPool::new();

            // Pre-populate pool with one item of each of the 100 types.
            populate_pool_with_all_types(&mut pool);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.len());
            }

            start.elapsed()
        });
    });

    // Test capacity_of() operation on a pool with many types
    let allocs_op = allocs.operation("capacity_of_many_types");
    group.bench_function("capacity_of_many_types", |b| {
        b.iter_custom(|iters| {
            let mut pool = BlindPool::new();

            // Pre-populate pool with one item of each of the 100 types.
            populate_pool_with_all_types(&mut pool);

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for _ in 0..iters {
                _ = black_box(pool.capacity_of::<Type050>());
            }

            start.elapsed()
        });
    });

    // Test removing items from a pool with many types
    let allocs_op = allocs.operation("remove_from_many_types");
    group.bench_function("remove_from_many_types", |b| {
        b.iter_custom(|iters| {
            let mut pools = iter::repeat_with(BlindPool::new)
                .take(usize::try_from(iters).unwrap())
                .collect::<Vec<_>>();

            // Pre-populate each pool and collect handles to remove.
            let pooled_items = pools
                .iter_mut()
                .map(|pool| {
                    populate_pool_with_all_types(pool);
                    // Insert an extra item that we'll remove.
                    pool.insert(Type025::with_value())
                })
                .collect::<Vec<_>>();

            let _span = allocs_op.measure_thread().iterations(iters);

            let start = Instant::now();

            for (pool, pooled) in pools.iter_mut().zip(pooled_items) {
                pool.remove(pooled);
            }

            start.elapsed()
        });
    });

    group.finish();

    allocs.print_to_stdout();
}
