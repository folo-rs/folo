//! Cross-crate schema round-trip canaries for the in-workspace engines.
//!
//! `alloc_tracker` and `all_the_time` each write their per-operation JSON with a
//! `Serialize` struct in their own crate, while the adapters in this crate read it
//! back with a separate `Deserialize` struct. Nothing but a matching field name
//! ties the two together, so a field renamed or dropped on one side drifts silently
//! — exactly the failure that made a live `collect` abort in CI when the producers
//! stopped writing the `mean_*` fields the adapters still required.
//!
//! These tests close that gap for the whole class: they drive the *real* producer
//! (not a committed fixture or a hand-rolled mock) and feed its output straight
//! through the adapter, so any future divergence between producer and consumer
//! fails the build instead of only surfacing against real output in CI. They cover
//! the single-span (no interval), multi-span (interval) and zero-iteration
//! (null slope, dropped as unmeasured) shapes the producer can emit.
//!
//! These integration tests exercise real producer files and processor clocks.

#![cfg(not(miri))]

use std::hint::black_box;

use alloc_tracker::Allocator;
use cbh_engines::{parse_all_the_time_operation, parse_alloc_tracker_operation};
use cbh_model::{BenchmarkResult, Metric, MetricKind};
use tempfile::tempdir;
use testing::DefaultAllocator;

#[global_allocator]
static ALLOCATOR: Allocator<DefaultAllocator> = Allocator::new(DefaultAllocator);

fn metric(record: &BenchmarkResult, kind: MetricKind) -> &Metric {
    record
        .metrics
        .iter()
        .find(|metric| metric.kind == kind)
        .unwrap_or_else(|| panic!("missing metric {kind:?}"))
}

#[test]
#[allow(clippy::float_cmp, reason = "the peak byte count is exact")]
#[cfg_attr(
    miri,
    ignore = "writes files, which is not supported under Miri isolation"
)]
fn alloc_tracker_single_span_output_parses() {
    let session = ::alloc_tracker::Session::new().no_stdout().no_file();
    {
        let operation = session.operation("roundtrip_single");
        let _span = operation.measure_thread().iterations(4);
        black_box(vec![0_u8; 64]);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_single.json")).unwrap();
    let record = parse_alloc_tracker_operation(&json)
        .expect("the adapter must parse real single-span alloc_tracker output")
        .expect("a single-span operation has a usable measurement");

    // Allocation metrics are present with a finite point estimate, and a
    // single span carries no confidence interval.
    let bytes = metric(&record, MetricKind::AllocatedBytes);
    assert!(bytes.value.is_finite(), "bytes value should be finite");
    assert_eq!(bytes.interval_low, None);
    assert_eq!(bytes.interval_high, None);

    let count = metric(&record, MetricKind::AllocationCount);
    assert!(count.value.is_finite(), "allocation count should be finite");
    assert_eq!(count.interval_low, None);
    assert_eq!(count.interval_high, None);

    let peak = metric(&record, MetricKind::PeakOutstandingBytes);
    assert_eq!(peak.value, 64.0);
    assert_eq!(peak.interval_low, None);
    assert_eq!(peak.interval_high, None);
}

#[test]
#[allow(
    clippy::float_cmp,
    reason = "the peak byte count and interval are exact"
)]
#[cfg_attr(
    miri,
    ignore = "writes files, which is not supported under Miri isolation"
)]
fn alloc_tracker_multi_span_output_carries_interval() {
    let session = ::alloc_tracker::Session::new().no_stdout().no_file();
    for _ in 0..4 {
        let operation = session.operation("roundtrip_multi");
        let _span = operation.measure_thread().iterations(4);
        black_box(vec![0_u8; 64]);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_multi.json")).unwrap();
    let record = parse_alloc_tracker_operation(&json)
        .expect("the adapter must parse real multi-span alloc_tracker output")
        .expect("a multi-span operation has a usable measurement");

    // Multiple spans clear the dispersion threshold, so the metrics carry an
    // interval — proving the adapter's interval field names still match the
    // producer's.
    let bytes = metric(&record, MetricKind::AllocatedBytes);
    assert!(bytes.interval_low.is_some(), "bytes interval low missing");
    assert!(bytes.interval_high.is_some(), "bytes interval high missing");

    let count = metric(&record, MetricKind::AllocationCount);
    assert!(count.interval_low.is_some(), "count interval low missing");
    assert!(count.interval_high.is_some(), "count interval high missing");

    let peak = metric(&record, MetricKind::PeakOutstandingBytes);
    assert_eq!(peak.value, 64.0);
    assert_eq!(peak.interval_low, Some(64.0));
    assert_eq!(peak.interval_high, Some(64.0));
}

#[test]
fn alloc_tracker_withheld_peaks_leave_other_metrics_usable() {
    for process_scope in [false, true] {
        let session = ::alloc_tracker::Session::new().no_stdout().no_file();
        let operation = session.operation("withheld");
        {
            let _span = operation.measure_thread().iterations(1);
            black_box(vec![0_u8; 64]);
        }
        if process_scope {
            drop(operation.measure_process().iterations(1));
        } else {
            drop(operation.measure_thread().no_peak().iterations(1));
        }
        {
            let _span = operation.measure_thread().iterations(1);
            black_box(vec![0_u8; 64]);
        }
        let report = session.to_report();
        assert_eq!(
            report
                .operations()
                .next()
                .unwrap()
                .1
                .peak_outstanding_bytes(),
            None
        );
        assert!(
            report.to_string().contains("n/a"),
            "the human report must not display a partial peak"
        );
        let directory = tempdir().unwrap();
        report.write_to_directory(directory.path());
        let json = std::fs::read_to_string(directory.path().join("withheld.json")).unwrap();
        let output: serde_json::Value = serde_json::from_str(&json).unwrap();
        for field in [
            "slope_peak_bytes",
            "interval_low_peak_bytes",
            "interval_high_peak_bytes",
        ] {
            assert!(output.get(field).is_none(), "withheld field {field}");
        }
        let record = parse_alloc_tracker_operation(&json).unwrap().unwrap();
        assert_eq!(record.metrics.len(), 2);
        assert!(metric(&record, MetricKind::AllocatedBytes).value > 0.0);
        assert!(metric(&record, MetricKind::AllocationCount).value > 0.0);
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reads the processor clock and writes files, unsupported under Miri isolation"
)]
fn all_the_time_single_span_output_parses() {
    let session = ::all_the_time::Session::new().no_stdout().no_file();
    {
        let operation = session.operation("roundtrip_single");
        let _span = operation.measure_thread().iterations(4);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_single.json")).unwrap();
    let record = parse_all_the_time_operation(&json)
        .expect("the adapter must parse real single-span all_the_time output")
        .expect("a single-span operation has a usable measurement");

    let processor_time = metric(&record, MetricKind::ProcessorTime);
    assert!(
        processor_time.value.is_finite(),
        "processor time should be finite"
    );
    assert_eq!(processor_time.interval_low, None);
    assert_eq!(processor_time.interval_high, None);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reads the processor clock and writes files, unsupported under Miri isolation"
)]
fn all_the_time_multi_span_output_carries_interval() {
    let session = ::all_the_time::Session::new().no_stdout().no_file();
    for _ in 0..4 {
        let operation = session.operation("roundtrip_multi");
        let _span = operation.measure_thread().iterations(4);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_multi.json")).unwrap();
    let record = parse_all_the_time_operation(&json)
        .expect("the adapter must parse real multi-span all_the_time output")
        .expect("a multi-span operation has a usable measurement");

    // Multiple spans clear the dispersion threshold, so the metric carries an
    // interval — proving the adapter's interval field names still match the
    // producer's.
    let processor_time = metric(&record, MetricKind::ProcessorTime);
    assert!(
        processor_time.interval_low.is_some(),
        "processor time interval low missing"
    );
    assert!(
        processor_time.interval_high.is_some(),
        "processor time interval high missing"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "writes files, which is not supported under Miri isolation"
)]
fn alloc_tracker_zero_iteration_output_is_skipped() {
    let session = ::alloc_tracker::Session::new().no_stdout().no_file();
    {
        let operation = session.operation("roundtrip_zero");
        // The workload could not run, so it records zero iterations; the producer
        // then writes null slopes (a NaN per-iteration rate).
        let _span = operation.measure_thread().iterations(0);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_zero.json")).unwrap();
    let record = parse_alloc_tracker_operation(&json)
        .expect("the adapter must parse real zero-iteration alloc_tracker output");
    // A zero-iteration operation has no usable measurement, so it is dropped rather
    // than carried into stored history as a non-finite value that could not
    // round-trip.
    assert!(
        record.is_none(),
        "a zero-iteration alloc_tracker operation must be skipped"
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "reads the processor clock and writes files, unsupported under Miri isolation"
)]
fn all_the_time_zero_iteration_output_is_skipped() {
    let session = ::all_the_time::Session::new().no_stdout().no_file();
    {
        let operation = session.operation("roundtrip_zero");
        // The workload could not run, so it records zero iterations; the producer
        // then writes a null slope (a NaN per-iteration rate).
        let _span = operation.measure_thread().iterations(0);
    }
    let directory = tempdir().unwrap();
    session.to_report().write_to_directory(directory.path());

    let json = std::fs::read_to_string(directory.path().join("roundtrip_zero.json")).unwrap();
    let record = parse_all_the_time_operation(&json)
        .expect("the adapter must parse real zero-iteration all_the_time output");
    // A zero-iteration operation has no usable measurement, so it is dropped rather
    // than carried into stored history as a non-finite value that could not
    // round-trip.
    assert!(
        record.is_none(),
        "a zero-iteration all_the_time operation must be skipped"
    );
}
