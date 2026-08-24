//! Integration test for `Report::collect()` with nonempty data.
//!
//! This test is in a separate integration test binary to avoid polluting
//! the global statics used by other tests.

#![allow(clippy::indexing_slicing, reason = "Panicking is acceptable in tests.")]

use std::thread;

use nm::{Event, Magnitude, Report};

/// The boundaries exercise explicit buckets, overflow, and cross-thread merging.
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
    const MAIN_COUNTER_OCCURRENCES: usize = 3;
    const FIRST_THREAD_COUNTER_OCCURRENCES: usize = 2;
    const SECOND_THREAD_COUNTER_OCCURRENCES: usize = 5;
    const MAIN_HISTOGRAM_OBSERVATIONS: &[Magnitude] = &[5, 75, 1000];
    const FIRST_THREAD_HISTOGRAM_OBSERVATIONS: &[Magnitude] = &[25, 200];
    const SECOND_THREAD_HISTOGRAM_OBSERVATIONS: &[Magnitude] = &[1, 2000];
    const EXPECTED_BUCKET_COUNTS: &[u64] = &[2, 1, 1, 1, 2];

    COUNTER_EVENT.with(|e| {
        e.batch(MAIN_COUNTER_OCCURRENCES).observe_once();
    });

    HISTOGRAM_EVENT.with(|e| {
        for &observation in MAIN_HISTOGRAM_OBSERVATIONS {
            e.observe(observation);
        }
    });

    thread::scope(|s| {
        s.spawn(|| {
            COUNTER_EVENT.with(|e| {
                e.batch(FIRST_THREAD_COUNTER_OCCURRENCES).observe_once();
            });

            HISTOGRAM_EVENT.with(|e| {
                for &observation in FIRST_THREAD_HISTOGRAM_OBSERVATIONS {
                    e.observe(observation);
                }
            });
        });

        s.spawn(|| {
            COUNTER_EVENT.with(|e| {
                e.batch(SECOND_THREAD_COUNTER_OCCURRENCES).observe_once();
            });

            HISTOGRAM_EVENT.with(|e| {
                for &observation in SECOND_THREAD_HISTOGRAM_OBSERVATIONS {
                    e.observe(observation);
                }
            });
        });
    });

    let expected_counter_count = u64::try_from(
        MAIN_COUNTER_OCCURRENCES
            + FIRST_THREAD_COUNTER_OCCURRENCES
            + SECOND_THREAD_COUNTER_OCCURRENCES,
    )
    .unwrap();
    let histogram_observations = MAIN_HISTOGRAM_OBSERVATIONS
        .iter()
        .chain(FIRST_THREAD_HISTOGRAM_OBSERVATIONS)
        .chain(SECOND_THREAD_HISTOGRAM_OBSERVATIONS);
    let expected_histogram_count = u64::try_from(histogram_observations.clone().count()).unwrap();
    let expected_histogram_sum = histogram_observations.copied().sum::<Magnitude>();
    let expected_histogram_mean = expected_histogram_sum
        .checked_div(Magnitude::try_from(expected_histogram_count).unwrap())
        .unwrap();

    let report = Report::collect();
    let event_names = report
        .events()
        .map(|event| event.name().as_ref())
        .collect::<Vec<_>>();
    assert_eq!(
        event_names,
        ["integration_test_counter", "integration_test_histogram"]
    );

    let counter_metrics = report
        .events()
        .find(|e| e.name() == "integration_test_counter")
        .unwrap();

    assert_eq!(counter_metrics.count(), expected_counter_count);
    assert_eq!(
        counter_metrics.sum(),
        Magnitude::try_from(expected_counter_count).unwrap()
    );
    assert_eq!(counter_metrics.mean(), 1);
    assert!(counter_metrics.histogram().is_none());

    let histogram_metrics = report
        .events()
        .find(|e| e.name() == "integration_test_histogram")
        .unwrap();

    assert_eq!(histogram_metrics.count(), expected_histogram_count);
    assert_eq!(histogram_metrics.sum(), expected_histogram_sum);
    assert_eq!(histogram_metrics.mean(), expected_histogram_mean);

    let histogram = histogram_metrics.histogram().unwrap();
    let buckets: Vec<_> = histogram.buckets().collect();

    let expected_buckets = TEST_BUCKETS
        .iter()
        .copied()
        .chain([Magnitude::MAX])
        .zip(EXPECTED_BUCKET_COUNTS.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(buckets, expected_buckets);

    let display_output = format!("{report}");

    assert!(display_output.contains("integration_test_counter"));
    assert!(display_output.contains("integration_test_histogram"));
    assert!(display_output.contains(&expected_counter_count.to_string()));
    assert!(display_output.contains(&expected_histogram_sum.to_string()));
    assert!(display_output.contains(&expected_histogram_mean.to_string()));
}
