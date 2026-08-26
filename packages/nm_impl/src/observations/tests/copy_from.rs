#![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

use crate::observations::DIRTY_BUCKETS_OVERFLOW_INDEX;
use crate::{Magnitude, ObservationBag, ObservationBagSync, Observations};

#[test]
fn copy_from_transfers_non_empty_bucket_counts() {
    let source = ObservationBag::new(&[-100, -10, 0, 10, 100]);

    // Insert observations into various buckets.
    source.insert(-1000, 1); // Goes into bucket 0 (le -100)
    source.insert(-50, 2); // Goes into bucket 1 (le -10)
    source.insert(0, 3); // Goes into bucket 2 (le 0)
    source.insert(5, 4); // Goes into bucket 3 (le 10)
    source.insert(50, 5); // Goes into bucket 4 (le 100)
    source.insert(1000, 6); // Goes outside any bucket (>100)

    let target = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    // Verify target starts empty.
    let snapshot_before = target.snapshot();
    assert_eq!(snapshot_before.count, 0);
    assert_eq!(snapshot_before.sum, 0);
    for &count in &snapshot_before.bucket_counts {
        assert_eq!(count, 0);
    }

    // Copy data from source to target.
    target.copy_from(&source);

    // Verify all data was transferred correctly. The count is written as the sum of the six
    // inserted observation counts so the total documents its own derivation; the sum below is
    // the corresponding magnitude-weighted total.
    let snapshot_after = target.snapshot();

    assert_eq!(snapshot_after.count, 1 + 2 + 3 + 4 + 5 + 6);
    assert_eq!(snapshot_after.sum, 5170);

    // Verify bucket counts.
    assert_eq!(snapshot_after.bucket_counts.len(), 5);
    assert_eq!(snapshot_after.bucket_counts[0], 1); // le -100
    assert_eq!(snapshot_after.bucket_counts[1], 2); // le -10
    assert_eq!(snapshot_after.bucket_counts[2], 3); // le 0
    assert_eq!(snapshot_after.bucket_counts[3], 4); // le 10
    assert_eq!(snapshot_after.bucket_counts[4], 5); // le 100
    // Note: observations with magnitude 1000 do not go into any bucket.
}

#[test]
fn copy_from_handles_repeated_observations_on_same_bucket() {
    // Observing the same bucket multiple times must leave that bucket dirty
    // exactly once - the dirty bit is set via `|=`, so subsequent observations
    // are idempotent with respect to the bitmap. A mutation that replaces `|`
    // with `^` would XOR the bit off on the second observation, causing
    // `copy_from` to skip the bucket even though its accumulated value changed.
    let source = ObservationBag::new(&[10]);
    let target = ObservationBagSync::new(&[10]);

    source.insert(5, 1);
    source.insert(5, 1);

    target.copy_from(&source);

    let snapshot = target.snapshot();
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 10);
    assert_eq!(snapshot.bucket_counts[0], 2);
}

#[test]
fn copy_from_handles_overflow_bucket_indices() {
    // Build a histogram with more buckets than the overflow threshold so that
    // observations targeting the overflow region exercise the catch-all branch in
    // `copy_from`. Sizing is derived from `DIRTY_BUCKETS_OVERFLOW_INDEX` so the test
    // tracks the storage layout: a handful of buckets beyond the boundary is enough
    // to place observations below, at, and above the overflow point.
    const BUCKETS_BEYOND_OVERFLOW: usize = 7;
    const BUCKET_COUNT: usize = DIRTY_BUCKETS_OVERFLOW_INDEX + BUCKETS_BEYOND_OVERFLOW;

    // Strictly increasing magnitudes so each insert lands in a distinct, known bucket.
    static MAGNITUDES: [Magnitude; BUCKET_COUNT] = {
        let mut arr = [0_i64; BUCKET_COUNT];
        let mut i = 0;
        while i < BUCKET_COUNT {
            #[expect(
                clippy::cast_possible_wrap,
                reason = "The small bucket index cannot wrap during conversion."
            )]
            let m = i as Magnitude;
            arr[i] = m;
            i += 1;
        }
        arr
    };

    // A bucket comfortably below the overflow boundary.
    const BELOW_OVERFLOW: usize = 10;

    let source = ObservationBag::new(&MAGNITUDES);
    let target = ObservationBagSync::new(&MAGNITUDES);

    // Observe one value below the overflow threshold and two at or above it,
    // so that both the non-overflow loop and the overflow scan must run.
    source.insert(MAGNITUDES[BELOW_OVERFLOW], 1);
    source.insert(MAGNITUDES[DIRTY_BUCKETS_OVERFLOW_INDEX], 2);
    source.insert(MAGNITUDES[BUCKET_COUNT - 1], 3);

    target.copy_from(&source);

    let snapshot = target.snapshot();
    assert_eq!(snapshot.count, 1 + 2 + 3);
    assert_eq!(snapshot.bucket_counts[BELOW_OVERFLOW], 1);
    assert_eq!(snapshot.bucket_counts[DIRTY_BUCKETS_OVERFLOW_INDEX], 2);
    assert_eq!(snapshot.bucket_counts[BUCKET_COUNT - 1], 3);

    // All other buckets must remain untouched - the overflow path must not
    // bleed writes into buckets that were never observed.
    for (i, &count) in snapshot.bucket_counts.iter().enumerate() {
        let expected = match i {
            BELOW_OVERFLOW => 1,
            DIRTY_BUCKETS_OVERFLOW_INDEX => 2,
            i if i == BUCKET_COUNT - 1 => 3,
            _ => 0,
        };
        assert_eq!(count, expected);
    }
}
