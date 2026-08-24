#![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

use crate::{ObservationBagSync, Observations};

#[test]
fn snapshot_merge_merges_data() {
    let observations = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations.insert(-1000, 1);
    observations.insert(0, 2);
    observations.insert(11, 3);
    observations.insert(1111, 4);

    // Reusing one snapshot isolates the merge arithmetic from fixture differences.
    let mut snapshot1 = observations.snapshot();
    let snapshot2 = observations.snapshot();

    snapshot1.merge_from(&snapshot2);

    assert_eq!(snapshot1.count, 2 * 10);
    assert_eq!(snapshot1.sum, 2 * (1111 * 4 + 11 * 3 - 1000));

    assert_eq!(snapshot1.bucket_counts.len(), 5);
    assert_eq!(snapshot1.bucket_counts[0], 2); // -1000
    assert_eq!(snapshot1.bucket_counts[1], 0); // nothing
    assert_eq!(snapshot1.bucket_counts[2], 4); // 0
    assert_eq!(snapshot1.bucket_counts[3], 0); // nothing
    assert_eq!(snapshot1.bucket_counts[4], 6); // 11
}

#[test]
fn snapshot_merges_directly_from_synchronized_bag() {
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations1.insert(-1000, 1);
    observations1.insert(0, 2);
    observations1.insert(11, 3);
    observations1.insert(1111, 4);

    let observations2 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations2.insert(-1000, 1);
    observations2.insert(0, 2);
    observations2.insert(11, 3);
    observations2.insert(1111, 4);

    let mut snapshot1 = observations1.snapshot();

    snapshot1.merge_from_observations(&observations2);

    assert_eq!(snapshot1.count, 2 * 10);
    assert_eq!(snapshot1.sum, 2 * (1111 * 4 + 11 * 3 - 1000));

    assert_eq!(snapshot1.bucket_counts.len(), 5);
    assert_eq!(snapshot1.bucket_counts[0], 2); // -1000
    assert_eq!(snapshot1.bucket_counts[1], 0); // nothing
    assert_eq!(snapshot1.bucket_counts[2], 4); // 0
    assert_eq!(snapshot1.bucket_counts[3], 0); // nothing
    assert_eq!(snapshot1.bucket_counts[4], 6); // 11
}

#[test]
fn bag_merge_merges_data_sync() {
    // Only the synchronized variant participates in cross-thread archival.
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations1.insert(-1000, 1);
    observations1.insert(0, 2);
    observations1.insert(11, 3);
    observations1.insert(1111, 4);

    let observations2 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);

    observations2.insert(-1000, 10);
    observations2.insert(0, 10);
    observations2.insert(11, 10);
    observations2.insert(1111, 10);

    observations1.merge_from(&observations2);

    let snapshot = observations1.snapshot();

    assert_eq!(snapshot.count, 10 + 40);
    assert_eq!(
        snapshot.sum,
        (1111 * 4 + 11 * 3 - 1000) + 10 * (1111 + 11 - 1000)
    );

    assert_eq!(snapshot.bucket_counts.len(), 5);
    assert_eq!(snapshot.bucket_counts[0], 11); // -1000
    assert_eq!(snapshot.bucket_counts[1], 0); // nothing
    assert_eq!(snapshot.bucket_counts[2], 12); // 0
    assert_eq!(snapshot.bucket_counts[3], 0); // nothing
    assert_eq!(snapshot.bucket_counts[4], 13); // 11
}

#[test]
#[should_panic]
fn snapshot_merge_with_mismatched_bucket_counts_panics() {
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
    let observations2 = ObservationBagSync::new(&[-100, -10, 0]);

    let mut snapshot1 = observations1.snapshot();
    let snapshot2 = observations2.snapshot();

    // Differing bucket counts are incompatible configurations.
    snapshot1.merge_from(&snapshot2);
}

#[test]
#[should_panic]
fn snapshot_merge_with_mismatched_bucket_magnitudes_panics() {
    // Same number of buckets, differing boundaries: the magnitude comparison
    // (not just length) must reject this.
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
    let observations2 = ObservationBagSync::new(&[-100, -10, 0, 20, 100]);

    let mut snapshot1 = observations1.snapshot();
    let snapshot2 = observations2.snapshot();

    snapshot1.merge_from(&snapshot2);
}

#[test]
#[should_panic]
fn bag_merge_with_mismatched_bucket_counts_panics() {
    // Only the synchronized variant participates in cross-thread archival.
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
    let observations2 = ObservationBagSync::new(&[-100, -10, 0]);

    // Differing bucket counts are incompatible configurations.
    observations1.merge_from(&observations2);
}

#[test]
#[should_panic]
fn bag_merge_with_mismatched_bucket_magnitudes_panics() {
    // Same number of buckets, differing boundaries: the magnitude comparison
    // (not just length) must reject this.
    let observations1 = ObservationBagSync::new(&[-100, -10, 0, 10, 100]);
    let observations2 = ObservationBagSync::new(&[-100, -10, 0, 20, 100]);

    observations1.merge_from(&observations2);
}
