//! Machine-readable JSON output of processor time statistics.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

use crate::Report;

/// Subdirectory of the Cargo target directory that receives the JSON files.
const OUTPUT_SUBDIRECTORY: &str = "all_the_time";

/// Machine-readable processor time statistics for a single operation.
#[derive(Serialize)]
struct OperationOutput<'a> {
    operation: &'a str,
    total_iterations: u64,
    total_processor_time_nanos: u64,
    span_count: u64,
    slope_processor_time_nanos: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    interval_low_processor_time_nanos: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    interval_high_processor_time_nanos: Option<f64>,
}

impl Report {
    /// Writes machine-readable JSON statistics into the Cargo target directory.
    ///
    /// One file is written per operation, named after the operation, at
    /// `<target>/all_the_time/<operation>.json`. Operation names are sanitized
    /// to be filesystem-safe and existing files are overwritten.
    ///
    /// The target directory is resolved the same way as Criterion (honoring
    /// `CARGO_TARGET_DIR`), falling back to a relative `target` directory.
    ///
    /// Writes nothing if no operations were captured. This may indicate that the
    /// session was part of a "list available benchmarks" probe run instead of
    /// some real activity.
    ///
    /// # Panics
    ///
    /// Panics if the output directory cannot be created or a file cannot be
    /// written. Benchmark results are not useful without the output files they
    /// produce, so a write failure is treated as fatal rather than recoverable.
    ///
    /// Also panics if two operation names sanitize to the same file name, since
    /// writing both would silently discard one operation's results.
    // Resolving Cargo's target and writing files requires integration coverage.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn write_to_target(&self) {
        let target =
            folo_utils::cargo_target_directory().unwrap_or_else(|| PathBuf::from("target"));
        self.write_to_directory(target.join(OUTPUT_SUBDIRECTORY));
    }

    /// Writes machine-readable JSON statistics into the given directory.
    ///
    /// One file is written per operation, named after the operation, as
    /// `<directory>/<operation>.json`. Operation names are sanitized to be
    /// filesystem-safe and existing files are overwritten. The directory is
    /// created if it does not exist.
    ///
    /// Writes nothing if no operations were captured.
    ///
    /// # Panics
    ///
    /// Panics if the output directory cannot be created or a file cannot be
    /// written. Benchmark results are not useful without the output files they
    /// produce, so a write failure is treated as fatal rather than recoverable.
    ///
    /// Also panics if two operation names sanitize to the same file name, since
    /// writing both would silently discard one operation's results.
    // Only filesystem effects live here; output preparation is unit-tested below.
    #[cfg_attr(test, mutants::skip)]
    pub fn write_to_directory(&self, directory: impl AsRef<Path>) {
        let directory = directory.as_ref();
        let outputs = self.output_files();

        // Probe runs must not create an empty directory.
        if outputs.is_empty() {
            return;
        }

        fs::create_dir_all(directory).unwrap_or_else(|error| {
            panic!(
                "failed to create benchmark output directory {}: {error}",
                directory.display()
            )
        });

        for (file_name, json) in outputs {
            let path = directory.join(file_name);
            fs::write(&path, json).unwrap_or_else(|error| {
                panic!(
                    "failed to write benchmark output file {}: {error}",
                    path.display()
                )
            });
        }
    }

    fn output_files(&self) -> Vec<(String, String)> {
        // Build every output up front, detecting sanitized-name collisions before
        // touching the filesystem. Two operation names that sanitize to the same
        // file name would otherwise silently overwrite each other's results.
        let mut file_names: HashMap<String, &str> = HashMap::new();
        let mut outputs = Vec::new();
        for (name, operation) in self.sorted_operations() {
            let Some(statistics) = operation.statistics() else {
                // Registered but never measured operations have no spans and thus
                // no statistics, so they leave no output file behind.
                continue;
            };

            let file_name = format!("{}.json", folo_utils::sanitize_file_name(name));
            if let Some(previous) = file_names.insert(file_name.clone(), name) {
                panic!(
                    "operations {previous:?} and {name:?} both map to the output file name \
                     {file_name:?} after sanitization; rename one of them to avoid silently \
                     overwriting benchmark results"
                );
            }

            let output = OperationOutput {
                operation: name,
                total_iterations: operation.total_iterations(),
                total_processor_time_nanos: duration_as_nanos(operation.total_processor_time()),
                span_count: statistics.span_count,
                slope_processor_time_nanos: statistics.slope_nanos,
                interval_low_processor_time_nanos: statistics.interval_nanos.map(|(low, _)| low),
                interval_high_processor_time_nanos: statistics.interval_nanos.map(|(_, high)| high),
            };

            let json = serde_json::to_string_pretty(&output)
                .expect("serializing fixed primitive fields to JSON cannot fail");

            outputs.push((file_name, json));
        }

        outputs
    }
}

/// Converts a [`Duration`] to whole nanoseconds, saturating at `u64::MAX`.
fn duration_as_nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::time::Duration;

    use serde_json::Value;

    use super::duration_as_nanos;
    use crate::pal::{FakePlatform, PlatformFacade};
    use crate::{Report, Session};

    fn single_output(report: &Report) -> (String, Value) {
        let outputs = report.output_files();
        assert_eq!(outputs.len(), 1);
        let (name, json) = outputs.into_iter().next().unwrap();
        (name, serde_json::from_str(&json).unwrap())
    }

    #[test]
    fn duration_as_nanos_converts_whole_nanoseconds() {
        assert_eq!(duration_as_nanos(Duration::from_millis(5)), 5_000_000);
    }

    #[test]
    fn duration_as_nanos_saturates_beyond_u64() {
        // `Duration::from_secs(u64::MAX)` holds far more nanoseconds than fit in
        // a `u64`, so the conversion saturates instead of panicking.
        assert_eq!(duration_as_nanos(Duration::from_secs(u64::MAX)), u64::MAX);
    }

    fn session_with_recorded_work(name: &str) -> Session {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform);

        fake_platform.set_thread_time(Duration::from_millis(0));
        {
            let operation = session.operation(name);
            let _span = operation.measure_thread().iterations(4);
            fake_platform.set_thread_time(Duration::from_millis(80));
        }

        session
    }

    #[test]
    fn prepares_operation_statistics_as_json() {
        let session = session_with_recorded_work("read_cell");
        let (name, value) = single_output(&session.to_report());
        assert_eq!(name, "read_cell.json");

        assert_eq!(
            value.get("operation").and_then(Value::as_str),
            Some("read_cell")
        );
        assert_eq!(
            value.get("total_iterations").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            value
                .get("total_processor_time_nanos")
                .and_then(Value::as_u64),
            Some(80_000_000)
        );
        // A single recorded span yields a span count of one and a slope equal to
        // the per-iteration mean, but no dispersion information, so the interval
        // fields are omitted.
        assert_eq!(value.get("span_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            value
                .get("slope_processor_time_nanos")
                .and_then(Value::as_f64),
            Some(20_000_000.0)
        );
        assert!(
            value.get("interval_low_processor_time_nanos").is_none(),
            "a single span carries no interval, so the field must be omitted"
        );
        assert!(
            value.get("interval_high_processor_time_nanos").is_none(),
            "a single span carries no interval, so the field must be omitted"
        );
        // Standard deviation, minimum, maximum and the raw mean are not emitted.
        assert!(value.get("mean_processor_time_nanos").is_none());
        assert!(value.get("std_dev_processor_time_nanos").is_none());
        assert!(value.get("min_processor_time_nanos").is_none());
        assert!(value.get("max_processor_time_nanos").is_none());
    }

    #[test]
    fn prepares_interval_when_multiple_spans_recorded() {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform);

        // Two spans at the same per-iteration rate collapse the interval onto the
        // slope, so both bounds are present and equal.
        {
            let operation = session.operation("read_cell");
            fake_platform.set_thread_time(Duration::from_millis(0));
            {
                let _span = operation.measure_thread().iterations(2);
                fake_platform.set_thread_time(Duration::from_millis(40));
            }
            {
                let _span = operation.measure_thread().iterations(2);
                fake_platform.set_thread_time(Duration::from_millis(80));
            }
        }

        let (_, value) = single_output(&session.to_report());
        assert_eq!(value.get("span_count").and_then(Value::as_u64), Some(2));
        assert_eq!(
            value
                .get("slope_processor_time_nanos")
                .and_then(Value::as_f64),
            Some(20_000_000.0)
        );
        assert_eq!(
            value
                .get("interval_low_processor_time_nanos")
                .and_then(Value::as_f64),
            Some(20_000_000.0)
        );
        assert_eq!(
            value
                .get("interval_high_processor_time_nanos")
                .and_then(Value::as_f64),
            Some(20_000_000.0)
        );
    }

    #[test]
    fn prepares_distinct_interval_bounds_and_every_measured_operation() {
        // JSON float parsing can round the last bit; tolerate only sub-nanosecond differences.
        const NANOS_TOLERANCE: f64 = 1.0;
        let platform = FakePlatform::new();
        let session = Session::with_platform(PlatformFacade::fake(platform.clone()));
        for (name, durations) in [
            (
                "alpha",
                [Duration::from_millis(20), Duration::from_millis(60)],
            ),
            (
                "beta",
                [Duration::from_millis(40), Duration::from_millis(80)],
            ),
        ] {
            let operation = session.operation(name);
            for duration in durations {
                platform.set_thread_time(Duration::ZERO);
                let _span = operation.measure_thread().iterations(2);
                platform.set_thread_time(duration);
            }
        }
        let report = session.to_report();
        let outputs = report.output_files();
        assert_eq!(outputs.len(), 2);
        for ((file, json), (name, operation)) in outputs.iter().zip(report.sorted_operations()) {
            let value: Value = serde_json::from_str(json).unwrap();
            let statistics = operation.statistics().unwrap();
            let (low, high) = statistics.interval_nanos.unwrap();
            assert!(low < high);
            assert_eq!(file, &format!("{name}.json"));
            assert_eq!(value.get("operation").unwrap(), name);
            assert_eq!(
                value.get("slope_processor_time_nanos").unwrap(),
                statistics.slope_nanos
            );
            assert!(
                (value
                    .get("interval_low_processor_time_nanos")
                    .unwrap()
                    .as_f64()
                    .unwrap()
                    - low)
                    .abs()
                    < NANOS_TOLERANCE
            );
            assert!(
                (value
                    .get("interval_high_processor_time_nanos")
                    .unwrap()
                    .as_f64()
                    .unwrap()
                    - high)
                    .abs()
                    < NANOS_TOLERANCE
            );
        }
    }

    #[test]
    fn prepares_null_slope_for_zero_iteration_operation() {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform);

        fake_platform.set_thread_time(Duration::from_millis(0));
        {
            let operation = session.operation("failed");
            // The workload could not run, so it records zero iterations.
            let _span = operation.measure_thread().iterations(0);
            fake_platform.set_thread_time(Duration::from_millis(80));
        }

        let (_, value) = single_output(&session.to_report());
        // A zero-iteration measurement has no per-iteration rate; the slope is
        // NaN, which serde_json renders as JSON null.
        assert!(
            value.get("slope_processor_time_nanos").unwrap().is_null(),
            "a zero-iteration slope must serialize as null"
        );
        assert_eq!(
            value.get("total_iterations").and_then(Value::as_u64),
            Some(0)
        );
    }

    #[test]
    fn sanitizes_operation_name_in_file_name() {
        let session = session_with_recorded_work("group/case name");
        let (name, value) = single_output(&session.to_report());
        assert_eq!(name, "group_case_name.json");

        // The original, unsanitized name is preserved inside the file.
        assert_eq!(
            value.get("operation").and_then(Value::as_str),
            Some("group/case name")
        );
    }

    #[test]
    fn empty_session_prepares_no_files() {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform);
        let session = Session::with_platform(platform);

        assert!(session.to_report().output_files().is_empty());
        _ = session.operation("unmeasured");
        assert!(session.to_report().output_files().is_empty());
    }

    #[test]
    fn skips_operations_without_spans() {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform);

        fake_platform.set_thread_time(Duration::from_millis(0));
        {
            let operation = session.operation("measured");
            let _span = operation.measure_thread().iterations(4);
            fake_platform.set_thread_time(Duration::from_millis(80));
        }
        // Registered but never measured, so it stays at zero iterations and must
        // be skipped rather than written.
        let _unmeasured = session.operation("unmeasured");

        let (name, _) = single_output(&session.to_report());
        assert_eq!(name, "measured.json");
    }

    #[test]
    #[should_panic]
    fn panics_when_operation_names_collide_after_sanitization() {
        let fake_platform = FakePlatform::new();
        let platform = PlatformFacade::fake(fake_platform.clone());
        let session = Session::with_platform(platform);

        // Both names sanitize to `group_case.json`, so writing both would silently
        // discard one operation's results.
        for name in ["group/case", "group_case"] {
            fake_platform.set_thread_time(Duration::from_millis(0));
            let operation = session.operation(name);
            let _span = operation.measure_thread().iterations(4);
            fake_platform.set_thread_time(Duration::from_millis(80));
        }

        _ = session.to_report().output_files();
    }
}
