#![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

use crate::{ObservationBag, ObservationBagSync, Observations};

#[test]
fn observations_are_recorded() {
    let observations = ObservationBag::new(&[]);

    // A quick sanity check first.
    observations.insert(7, 2);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);

    // Zero is a perfectly fine magnitude.
    observations.insert(0, 3);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 5);
    assert_eq!(snapshot.sum, 14);

    // Negative magnitudes are also fine.
    observations.insert(-30, 4);

    let snapshot = observations.snapshot();
    assert_eq!(snapshot.count, 9);
    assert_eq!(snapshot.sum, -106);
}

#[test]
fn observations_are_recorded_sync() {
    let observations = ObservationBagSync::new(&[]);

    // A quick sanity check first.
    observations.insert(7, 2);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);

    // Zero is a perfectly fine magnitude.
    observations.insert(0, 3);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 5);
    assert_eq!(snapshot.sum, 14);

    // Negative magnitudes are also fine.
    observations.insert(-30, 4);

    let snapshot = observations.snapshot();
    assert_eq!(snapshot.count, 9);
    assert_eq!(snapshot.sum, -106);
}

#[test]
fn observations_are_recorded_in_histogram() {
    let observations = ObservationBag::new(&[-100, -10, 0, 10, 100]);

    observations.insert(-1000, 1);
    observations.insert(0, 2);
    observations.insert(11, 3);
    observations.insert(1111, 4);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 10);
    assert_eq!(snapshot.sum, 1111 * 4 + 11 * 3 - 1000);

    assert_eq!(snapshot.bucket_counts.len(), 5);
    assert_eq!(snapshot.bucket_counts[0], 1); // -1000
    assert_eq!(snapshot.bucket_counts[1], 0); // nothing
    assert_eq!(snapshot.bucket_counts[2], 2); // 0
    assert_eq!(snapshot.bucket_counts[3], 0); // nothing
    assert_eq!(snapshot.bucket_counts[4], 3); // 11

    // 1111 is outside any bucket ranges, so only present in the totals.
}

#[test]
fn observations_are_recorded_in_histogram_sync() {
    let observations = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations.insert(-1000, 1);
    observations.insert(0, 2);
    observations.insert(11, 3);
    observations.insert(1111, 4);

    let snapshot = observations.snapshot();

    assert_eq!(snapshot.count, 10);
    assert_eq!(snapshot.sum, 1111 * 4 + 11 * 3 - 1000);

    assert_eq!(snapshot.bucket_counts.len(), 5);
    assert_eq!(snapshot.bucket_counts[0], 1); // -1000
    assert_eq!(snapshot.bucket_counts[1], 0); // nothing
    assert_eq!(snapshot.bucket_counts[2], 2); // 0
    assert_eq!(snapshot.bucket_counts[3], 0); // nothing
    assert_eq!(snapshot.bucket_counts[4], 3); // 11

    // 1111 is outside any bucket ranges, so only present in the totals.
}

#[test]
fn existing_snapshots_do_not_change() {
    let observations = ObservationBag::new(&[]);
    observations.insert(7, 2);

    let snapshot = observations.snapshot();
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);

    observations.insert(123, 123);

    // The existing snapshot must not have changed.
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);
}

#[test]
fn existing_snapshots_do_not_change_sync() {
    let observations = ObservationBagSync::new(&[]);
    observations.insert(7, 2);

    let snapshot = observations.snapshot();
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);

    observations.insert(123, 123);

    // The existing snapshot must not have changed.
    assert_eq!(snapshot.count, 2);
    assert_eq!(snapshot.sum, 14);
}

#[test]
fn insert_with_zero_count_does_not_mark_dirty() {
    // A no-op observation must not pollute the dirty-bucket bitmap. Otherwise
    // a later `copy_from` would perform a redundant atomic store for a bucket
    // whose value never changed.
    let bag = ObservationBag::new(&[10]);

    bag.insert(5, 0);

    assert_eq!(bag.count(), 0);
    assert_eq!(bag.take_dirty_buckets(), 0);
    let snapshot = bag.snapshot();
    assert_eq!(snapshot.bucket_counts[0], 0);
}
