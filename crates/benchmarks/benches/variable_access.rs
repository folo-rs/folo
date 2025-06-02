//! We compare the overhead of accessing different types of variables.

#![allow(
    missing_docs,
    reason = "No need for API documentation in benchmark code"
)]
#![allow(clippy::mutex_integer, reason = "Dummy data, does not matter")]

use std::{
    cell::{Cell, LazyCell, OnceCell, RefCell, UnsafeCell},
    hint::black_box,
    rc::{Rc, Weak as RcWeak},
    sync::{
        Arc, LazyLock, Mutex, OnceLock, RwLock, Weak as ArcWeak,
        atomic::{self, AtomicU64},
    },
};

use criterion::{Criterion, criterion_group, criterion_main};

criterion_group!(benches, entrypoint);
criterion_main!(benches);

const EXPECTED_VALUE: u64 = 0x1122334411223344;

fn entrypoint(c: &mut Criterion) {
    let mut group = c.benchmark_group("variable_access");

    group.bench_function("static_immutable", |b| {
        b.iter(|| {
            static VAR: u64 = EXPECTED_VALUE;
            assert_eq!(black_box(VAR), EXPECTED_VALUE);
        });
    });

    group.bench_function("static_lazy_lock", |b| {
        b.iter(|| {
            static VAR: LazyLock<u64> = LazyLock::new(|| EXPECTED_VALUE);
            assert_eq!(black_box(*VAR), EXPECTED_VALUE);
        });
    });

    group.bench_function("static_once_lock", |b| {
        b.iter(|| {
            static VAR: OnceLock<u64> = OnceLock::new();
            assert_eq!(
                black_box(*VAR.get_or_init(|| EXPECTED_VALUE)),
                EXPECTED_VALUE
            );
        });
    });

    group.bench_function("thread_local_immutable", |b| {
        b.iter(|| {
            thread_local!(static VAR: u64 = const { EXPECTED_VALUE });
            assert_eq!(black_box(VAR.with(|v| *v)), EXPECTED_VALUE);
        });
    });

    group.bench_function("thread_local_refcell_borrow", |b| {
        b.iter(|| {
            thread_local!(static VAR: RefCell<u64> = const { RefCell::new(EXPECTED_VALUE) });
            assert_eq!(black_box(VAR.with_borrow(|v| *v)), EXPECTED_VALUE);
        });
    });

    group.bench_function("thread_local_refcell_borrow_mut", |b| {
        b.iter(|| {
            thread_local!(static VAR: RefCell<u64> = const { RefCell::new(EXPECTED_VALUE) });
            assert_eq!(black_box(VAR.with_borrow_mut(|v| *v)), EXPECTED_VALUE);
        });
    });

    group.bench_function("thread_local_lazy_cell", |b| {
        b.iter(|| {
            thread_local!(static VAR: LazyCell<u64> = LazyCell::new(|| EXPECTED_VALUE));
            assert_eq!(black_box(VAR.with(|v| **v)), EXPECTED_VALUE);
        });
    });

    group.bench_function("thread_local_once_cell", |b| {
        b.iter(|| {
            thread_local!(static VAR: OnceCell<u64> = const { OnceCell::new() });
            assert_eq!(
                black_box(VAR.with(|v| *v.get_or_init(|| EXPECTED_VALUE))),
                EXPECTED_VALUE
            );
        });
    });

    // We lift these out of the iteration function to avoid measuring the cost of field
    // initialization - all we care about is the reading from the field.

    let local_immutable: u64 = black_box(EXPECTED_VALUE);

    group.bench_function("local_immutable", |b| {
        b.iter(|| assert_eq!(black_box(local_immutable), EXPECTED_VALUE));
    });

    let mut local_mutable: u64 = black_box(EXPECTED_VALUE);

    group.bench_function("local_mutable", |b| {
        b.iter(|| assert_eq!(black_box(*black_box(&mut local_mutable)), EXPECTED_VALUE));
    });

    let local_cell: Cell<u64> = black_box(Cell::new(EXPECTED_VALUE));

    group.bench_function("local_cell", |b| {
        b.iter(|| assert_eq!(black_box(local_cell.get()), EXPECTED_VALUE));
    });

    let local_refcell: RefCell<u64> = black_box(RefCell::new(EXPECTED_VALUE));

    group.bench_function("local_refcell_borrow", |b| {
        b.iter(|| assert_eq!(black_box(*local_refcell.borrow()), EXPECTED_VALUE));
    });

    group.bench_function("local_refcell_borrow_mut", |b| {
        b.iter(|| assert_eq!(black_box(*local_refcell.borrow_mut()), EXPECTED_VALUE));
    });

    let local_unsafe_cell: UnsafeCell<u64> = black_box(UnsafeCell::new(EXPECTED_VALUE));

    group.bench_function("local_unsafe_cell", |b| {
        b.iter(|| {
            assert_eq!(
                // SAFETY: It's all good.
                black_box(unsafe { *local_unsafe_cell.get() }),
                EXPECTED_VALUE
            );
        });
    });

    let local_lazy_cell: LazyCell<u64> = black_box(LazyCell::new(|| EXPECTED_VALUE));

    group.bench_function("local_lazy_cell", |b| {
        b.iter(|| assert_eq!(black_box(*local_lazy_cell), EXPECTED_VALUE));
    });

    let local_once_cell: OnceCell<u64> = black_box(OnceCell::new());

    group.bench_function("local_once_cell", |b| {
        b.iter(|| {
            assert_eq!(
                black_box(*local_once_cell.get_or_init(|| EXPECTED_VALUE)),
                EXPECTED_VALUE
            );
        });
    });

    let local_mutex: Mutex<u64> = black_box(Mutex::new(EXPECTED_VALUE));

    group.bench_function("local_mutex", |b| {
        b.iter(|| {
            assert_eq!(black_box(*local_mutex.lock().unwrap()), EXPECTED_VALUE);
        });
    });

    let local_rwlock: RwLock<u64> = black_box(RwLock::new(EXPECTED_VALUE));

    group.bench_function("local_rwlock_read", |b| {
        b.iter(|| {
            assert_eq!(black_box(*local_rwlock.read().unwrap()), EXPECTED_VALUE);
        });
    });

    group.bench_function("local_rwlock_write", |b| {
        b.iter(|| {
            assert_eq!(black_box(*local_rwlock.write().unwrap()), EXPECTED_VALUE);
        });
    });

    // Reference counted smart pointers benchmarks
    let local_arc: Arc<u64> = black_box(Arc::new(EXPECTED_VALUE));

    group.bench_function("local_arc", |b| {
        b.iter(|| assert_eq!(black_box(*local_arc), EXPECTED_VALUE));
    });

    let local_arc_weak: ArcWeak<u64> = black_box(Arc::downgrade(&local_arc));

    group.bench_function("local_arc_weak_upgrade", |b| {
        b.iter(|| {
            let strong = black_box(local_arc_weak.upgrade().unwrap());
            assert_eq!(black_box(*strong), EXPECTED_VALUE);
        });
    });

    let local_rc: Rc<u64> = black_box(Rc::new(EXPECTED_VALUE));

    group.bench_function("local_rc", |b| {
        b.iter(|| assert_eq!(black_box(*local_rc), EXPECTED_VALUE));
    });

    let local_rc_weak: RcWeak<u64> = black_box(Rc::downgrade(&local_rc));

    group.bench_function("local_rc_weak_upgrade", |b| {
        b.iter(|| {
            let strong = black_box(local_rc_weak.upgrade().unwrap());
            assert_eq!(black_box(*strong), EXPECTED_VALUE);
        });
    });

    let atomic = AtomicU64::new(EXPECTED_VALUE);

    group.bench_function("local_atomic_relaxed", |b| {
        b.iter(|| {
            assert_eq!(
                black_box(atomic.load(atomic::Ordering::Relaxed)),
                EXPECTED_VALUE
            );
        });
    });

    group.bench_function("local_atomic_acquire", |b| {
        b.iter(|| {
            assert_eq!(
                black_box(atomic.load(atomic::Ordering::Acquire)),
                EXPECTED_VALUE
            );
        });
    });

    group.finish();
}
