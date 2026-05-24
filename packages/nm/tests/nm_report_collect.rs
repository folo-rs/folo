//! Integration test for `Report::collect()` with nonempty data.
//!
//! This test is in a separate integration test binary to avoid polluting
//! the global statics used by other tests.

#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

use std::thread;

use nm::{Event, Magnitude, Report};

/// Histogram buckets used for testing.
const TEST_BUCKETS: &[Magnitude] = &[10, 50, 100, 500];

thread_local! {
    static COUNTER_EVENT: Event = Event::builder()
        .name("integration_test_counter")
        .build();

    static HISTOGRAM_EVENT: Event = Event::builder()
        .name("integration_test_histogram")
        .histogram(TEST_BUCKETS)
        .build();
}

#[test]
fn report_collect_aggregates_data_from_multiple_threads() {
    // Record observations on the main thread.
    COUNTER_EVENT.with(|e| {
        e.observe_once();
        e.observe_once();
        e.observe_once();
    });

    HISTOGRAM_EVENT.with(|e| {
        e.observe(5); // Goes into bucket le 10
        e.observe(75); // Goes into bucket le 100
        e.observe(1000); // Goes into +inf bucket
    });

    // Record observations on additional threads.
    thread::scope(|s| {
        s.spawn(|| {
            COUNTER_EVENT.with(|e| {
                e.observe_once();
                e.observe_once();
            });

            HISTOGRAM_EVENT.with(|e| {
                e.observe(25); // Goes into bucket le 50
                e.observe(200); // Goes into bucket le 500
            });
        });

        s.spawn(|| {
            COUNTER_EVENT.with(|e| {
                e.batch(5).observe_once();
            });

            HISTOGRAM_EVENT.with(|e| {
                e.observe(1); // Goes into bucket le 10
                e.observe(2000); // Goes into +inf bucket
            });
        });
    });

    // Collect the report.
    let report = Report::collect();

    // Find and verify the counter event.
    let counter_metrics = report
        .events()
        .find(|e| e.name() == "integration_test_counter")
        .unwrap();

    // Total count: 3 (main) + 2 (thread 1) + 5 (thread 2) = 10
    assert_eq!(counter_metrics.count(), 10);
    // Sum equals count for observe_once (magnitude 1).
    assert_eq!(counter_metrics.sum(), 10);
    // Mean should be 1.
    assert_eq!(counter_metrics.mean(), 1);
    // No histogram configured.
    assert!(counter_metrics.histogram().is_none());

    // Find and verify the histogram event.
    let histogram_metrics = report
        .events()
        .find(|e| e.name() == "integration_test_histogram")
        .unwrap();

    // Total count: 3 (main) + 2 (thread 1) + 2 (thread 2) = 7
    assert_eq!(histogram_metrics.count(), 7);

    // Sum: 5 + 75 + 1000 + 25 + 200 + 1 + 2000 = 3306
    assert_eq!(histogram_metrics.sum(), 3306);

    // Mean: 3306 / 7 = 472 (integer division)
    assert_eq!(histogram_metrics.mean(), 472);

    // Verify histogram is present and has correct structure.
    let histogram = histogram_metrics.histogram().unwrap();

    // Collect bucket data.
    let buckets: Vec<_> = histogram.buckets().collect();

    // Buckets: [10, 50, 100, 500, MAX]
    assert_eq!(buckets.len(), 5);

    // Bucket le 10: 5 (main) + 1 (thread 2) = 2
    assert_eq!(buckets[0], (10, 2));

    // Bucket le 50: 25 (thread 1) = 1
    assert_eq!(buckets[1], (50, 1));

    // Bucket le 100: 75 (main) = 1
    assert_eq!(buckets[2], (100, 1));

    // Bucket le 500: 200 (thread 1) = 1
    assert_eq!(buckets[3], (500, 1));

    // Bucket +inf: 1000 (main) + 2000 (thread 2) = 2
    assert_eq!(buckets[4], (Magnitude::MAX, 2));

    // Verify the report display contains the expected event names and critical numbers.
    let display_output = format!("{report}");

    assert!(display_output.contains("integration_test_counter"));
    assert!(display_output.contains("integration_test_histogram"));
    // Critical numbers should be present.
    assert!(display_output.contains("10")); // Counter count
    assert!(display_output.contains("3306")); // Histogram sum
    assert!(display_output.contains("472")); // Histogram mean
}
