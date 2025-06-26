use criterion::{Criterion, criterion_group, criterion_main};

use blind_pool::BlindPool;

fn insert_remove_same_type(c: &mut Criterion) {
    c.bench_function("blind_pool insert/remove u64", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            let handles: Vec<_> = (0..1000).map(|i| pool.insert(i as u64)).collect();
            for handle in handles {
                pool.remove(handle);
            }
        });
    });
}

fn insert_remove_different_types(c: &mut Criterion) {
    c.bench_function("blind_pool insert/remove mixed types", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();

            let mut handles = Vec::new();
            for i in 0..250 {
                handles.push(pool.insert(i as u32).erase());
                handles.push(pool.insert(i as u64).erase());
                handles.push(pool.insert(i as f32).erase());
                handles.push(pool.insert(i as i64).erase()); // Changed from String to i64
            }

            for handle in handles {
                pool.remove(handle);
            }
        });
    });
}

fn insert_only_same_type(c: &mut Criterion) {
    c.bench_function("blind_pool insert u64 only", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0..1000 {
                let _ = pool.insert(i as u64);
            }
        });
    });
}

fn insert_only_different_types(c: &mut Criterion) {
    c.bench_function("blind_pool insert mixed types only", |b| {
        b.iter(|| {
            let mut pool = BlindPool::new();
            for i in 0..250 {
                let _ = pool.insert(i as u32);
                let _ = pool.insert(i as u64);
                let _ = pool.insert(i as f32);
                let _ = pool.insert(i as i64); // Changed from String to i64
            }
        });
    });
}

criterion_group!(
    benches,
    insert_remove_same_type,
    insert_remove_different_types,
    insert_only_same_type,
    insert_only_different_types
);
criterion_main!(benches);
