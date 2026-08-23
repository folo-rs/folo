#![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

use std::sync::Arc;
use std::{iter, thread};

use static_assertions::assert_impl_all;
use testing::with_watchdog;

use crate::{Magnitude, ObservationBagSync, Observations};

// The sync bag is the only observation storage shared across threads, so it must be
// both `Send` and `Sync`. Ref: nm implementation documentation, "Report assembly".
assert_impl_all!(ObservationBagSync: Send, Sync);

// The concurrent tests below are deterministic: every spawned thread performs a fixed,
// bounded number of operations and is joined before assertions run, so there is no
// reliance on timing or a real-time clock. They are also structured to run under Miri
// (via miri-harder) to detect data races on the atomic operations.

#[test]
fn sync_concurrent_inserts_accumulate_correctly() {
    with_watchdog(|| {
        // Multiple writers observing the same magnitude must accumulate without lost updates.
        // A handful of threads each performing several inserts is enough to interleave the
        // atomic read-modify-write sequences that this property depends on.
        const THREADS: usize = 4;
        const INSERTS_PER_THREAD: usize = 10;
        const MAGNITUDE: Magnitude = 7;

        let bag = Arc::new(ObservationBagSync::new(&[]));

        let handles: Vec<_> = iter::repeat_with(|| {
            thread::spawn({
                let bag = Arc::clone(&bag);
                move || {
                    for _ in 0..INSERTS_PER_THREAD {
                        bag.insert(MAGNITUDE, 1);
                    }
                }
            })
        })
        .take(THREADS)
        .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = bag.snapshot();
        assert_eq!(snapshot.count, (THREADS * INSERTS_PER_THREAD) as u64);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "The bounded test workload cannot wrap during conversion."
        )]
        let expected_sum = (THREADS * INSERTS_PER_THREAD) as i64 * MAGNITUDE;
        assert_eq!(snapshot.sum, expected_sum);
    });
}

#[test]
fn sync_concurrent_inserts_with_histogram_accumulate_correctly() {
    with_watchdog(|| {
        // As above, but with a histogram configuration so the per-bucket atomic increments
        // are also exercised under contention.
        const THREADS: usize = 4;
        const INSERTS_PER_THREAD: usize = 10;

        let bag = Arc::new(ObservationBagSync::new(&[-100, -10, 0, 10, 100]));

        let handles: Vec<_> = iter::repeat_with(|| {
            thread::spawn({
                let bag = Arc::clone(&bag);
                move || {
                    for _ in 0..INSERTS_PER_THREAD {
                        // Magnitude 5 should land in the "le 10" bucket (index 3).
                        bag.insert(5, 1);
                    }
                }
            })
        })
        .take(THREADS)
        .collect();

        for handle in handles {
            handle.join().unwrap();
        }

        let snapshot = bag.snapshot();
        let total = (THREADS * INSERTS_PER_THREAD) as u64;
        assert_eq!(snapshot.count, total);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "The bounded test workload cannot wrap during conversion."
        )]
        let total_i64 = total as i64;
        assert_eq!(snapshot.sum, total_i64 * 5);
        assert_eq!(snapshot.bucket_counts[3], total);
    });
}

#[test]
fn sync_concurrent_insert_and_snapshot() {
    with_watchdog(|| {
        // One thread inserts observations while another takes snapshots concurrently.
        // This exercises the read/write paths that `ObservationBagSync` is designed for;
        // Miri detects any data races on the atomic operations. The reader's snapshots
        // are intentionally discarded - only race-freedom and the final consistent state
        // are under test.
        const OBSERVATIONS: u64 = 20;
        const MAGNITUDE: Magnitude = 5;

        let bag = Arc::new(ObservationBagSync::new(&[-100, -10, 0, 10, 100]));

        let writer = thread::spawn({
            let bag = Arc::clone(&bag);
            move || {
                for _ in 0..OBSERVATIONS {
                    bag.insert(MAGNITUDE, 1);
                }
            }
        });

        let reader = thread::spawn({
            let bag = Arc::clone(&bag);
            move || {
                for _ in 0..OBSERVATIONS {
                    _ = bag.snapshot();
                }
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // After both threads complete, the final snapshot must be fully consistent.
        let snapshot = bag.snapshot();
        assert_eq!(snapshot.count, OBSERVATIONS);
        #[expect(
            clippy::cast_possible_wrap,
            reason = "The bounded test workload cannot wrap during conversion."
        )]
        let expected_sum = OBSERVATIONS as i64 * MAGNITUDE;
        assert_eq!(snapshot.sum, expected_sum);
    });
}

#[test]
fn sync_concurrent_merge_from_while_inserting() {
    with_watchdog(|| {
        // One thread inserts into a source bag while another merges from the source into a
        // target. This exercises concurrent reads on the source bag via `merge_from`. Both
        // threads perform a fixed number of iterations, so the test terminates deterministically;
        // the property under test is race-freedom, verified by Miri.
        const INSERTS: usize = 20;
        const MERGES: usize = 5;

        let source = Arc::new(ObservationBagSync::new(&[-100, 0, 100]));
        let target = Arc::new(ObservationBagSync::new(&[-100, 0, 100]));

        let writer = thread::spawn({
            let source = Arc::clone(&source);
            move || {
                for _ in 0..INSERTS {
                    source.insert(5, 1);
                }
            }
        });

        let merger = thread::spawn({
            let source = Arc::clone(&source);
            let target = Arc::clone(&target);
            move || {
                for _ in 0..MERGES {
                    target.merge_from(&source);
                }
            }
        });

        writer.join().unwrap();
        merger.join().unwrap();
    });
}
