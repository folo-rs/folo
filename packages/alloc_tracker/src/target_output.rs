//! Machine-readable JSON output of memory allocation statistics.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::Report;

/// Subdirectory of the Cargo target directory that receives the JSON files.
const OUTPUT_SUBDIRECTORY: &str = "alloc_tracker";

/// Machine-readable allocation statistics for a single operation.
///
/// Carries both the pooled per-iteration means and the warmup-robust dispersion
/// (slope, standard deviation, bootstrap interval and extremes) for each metric,
/// mirroring the shape `all_the_time` writes for processor time.
#[derive(Serialize)]
struct OperationOutput<'a> {
    operation: &'a str,
    total_iterations: u64,
    total_bytes_allocated: u64,
    total_allocations_count: u64,
    mean_bytes_per_iteration: u64,
    mean_allocations_per_iteration: u64,
    span_count: u64,
    slope_bytes_per_iteration: f64,
    std_dev_bytes_per_iteration: f64,
    interval_low_bytes_per_iteration: f64,
    interval_high_bytes_per_iteration: f64,
    min_bytes_per_iteration: f64,
    max_bytes_per_iteration: f64,
    slope_allocations_per_iteration: f64,
    std_dev_allocations_per_iteration: f64,
    interval_low_allocations_per_iteration: f64,
    interval_high_allocations_per_iteration: f64,
    min_allocations_per_iteration: f64,
    max_allocations_per_iteration: f64,
}

impl Report {
    /// Writes machine-readable JSON statistics into the Cargo target directory.
    ///
    /// One file is written per operation, named after the operation, at
    /// `<target>/alloc_tracker/<operation>.json`. Operation names are sanitized
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
    pub(crate) fn write_to_directory(&self, directory: impl AsRef<Path>) {
        let directory = directory.as_ref();

        // Build every output up front, detecting sanitized-name collisions before
        // touching the filesystem. Two operation names that sanitize to the same
        // file name would otherwise silently overwrite each other's results.
        let mut file_names: HashMap<String, &str> = HashMap::new();
        let mut outputs: Vec<(PathBuf, String)> = Vec::new();
        for (name, operation) in self.operations() {
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
                total_bytes_allocated: operation.total_bytes_allocated(),
                total_allocations_count: operation.total_allocations_count(),
                mean_bytes_per_iteration: operation.mean_bytes(),
                mean_allocations_per_iteration: operation.mean_allocations(),
                span_count: statistics.span_count,
                slope_bytes_per_iteration: statistics.bytes.slope,
                std_dev_bytes_per_iteration: statistics.bytes.std_dev,
                interval_low_bytes_per_iteration: statistics.bytes.interval_low,
                interval_high_bytes_per_iteration: statistics.bytes.interval_high,
                min_bytes_per_iteration: statistics.bytes.min,
                max_bytes_per_iteration: statistics.bytes.max,
                slope_allocations_per_iteration: statistics.allocations.slope,
                std_dev_allocations_per_iteration: statistics.allocations.std_dev,
                interval_low_allocations_per_iteration: statistics.allocations.interval_low,
                interval_high_allocations_per_iteration: statistics.allocations.interval_high,
                min_allocations_per_iteration: statistics.allocations.min,
                max_allocations_per_iteration: statistics.allocations.max,
            };

            let json = serde_json::to_string_pretty(&output)
                .expect("serializing fixed primitive fields to JSON cannot fail");

            outputs.push((directory.join(file_name), json));
        }

        // Without any output, no directory is created, so a probe run that captured
        // no measurable work leaves nothing behind.
        if outputs.is_empty() {
            return;
        }

        fs::create_dir_all(directory).unwrap_or_else(|error| {
            panic!(
                "failed to create benchmark output directory {}: {error}",
                directory.display()
            )
        });

        for (path, json) in outputs {
            fs::write(&path, json).unwrap_or_else(|error| {
                panic!(
                    "failed to write benchmark output file {}: {error}",
                    path.display()
                )
            });
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;
    use std::path::Path;

    use serde_json::Value;

    use crate::Session;
    use crate::allocator::register_fake_allocation;

    fn read_json(path: &Path) -> Value {
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
    }

    fn session_with_recorded_work(name: &str) -> Session {
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation(name);
            let _span = operation.measure_thread().iterations(4);
            register_fake_allocation(800, 8);
        }
        session
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn writes_operation_statistics_as_json() {
        let session = session_with_recorded_work("allocate_vec");
        let directory = tempfile::tempdir().unwrap();

        session.to_report().write_to_directory(directory.path());

        let file = directory.path().join("allocate_vec.json");
        let value = read_json(&file);

        assert_eq!(
            value.get("operation").and_then(Value::as_str),
            Some("allocate_vec")
        );
        assert_eq!(
            value.get("total_iterations").and_then(Value::as_u64),
            Some(4)
        );
        assert_eq!(
            value.get("total_bytes_allocated").and_then(Value::as_u64),
            Some(800)
        );
        assert_eq!(
            value.get("total_allocations_count").and_then(Value::as_u64),
            Some(8)
        );
        assert_eq!(
            value
                .get("mean_bytes_per_iteration")
                .and_then(Value::as_u64),
            Some(200)
        );
        assert_eq!(
            value
                .get("mean_allocations_per_iteration")
                .and_then(Value::as_u64),
            Some(2)
        );
        // A single recorded span yields a span count of one, per-metric slopes
        // equal to the per-iteration means, and degenerate intervals that
        // collapse onto them.
        assert_eq!(value.get("span_count").and_then(Value::as_u64), Some(1));
        assert_eq!(
            value
                .get("slope_bytes_per_iteration")
                .and_then(Value::as_f64),
            Some(200.0)
        );
        assert_eq!(
            value
                .get("interval_low_bytes_per_iteration")
                .and_then(Value::as_f64),
            Some(200.0)
        );
        assert_eq!(
            value
                .get("interval_high_bytes_per_iteration")
                .and_then(Value::as_f64),
            Some(200.0)
        );
        assert_eq!(
            value
                .get("std_dev_bytes_per_iteration")
                .and_then(Value::as_f64),
            Some(0.0)
        );
        assert_eq!(
            value
                .get("slope_allocations_per_iteration")
                .and_then(Value::as_f64),
            Some(2.0)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn sanitizes_operation_name_in_file_name() {
        let session = session_with_recorded_work("group/case name");
        let directory = tempfile::tempdir().unwrap();

        session.to_report().write_to_directory(directory.path());

        let file = directory.path().join("group_case_name.json");
        assert!(file.exists());

        // The original, unsanitized name is preserved inside the file.
        assert_eq!(
            read_json(&file).get("operation").and_then(Value::as_str),
            Some("group/case name")
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn empty_session_writes_no_files() {
        let session = Session::new().no_stdout().no_file();
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("nested");

        session.to_report().write_to_directory(&target);

        // Nothing is written, so the directory is not even created.
        assert!(!target.exists());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn skips_operations_without_iterations() {
        let session = Session::new().no_stdout().no_file();
        {
            let operation = session.operation("measured");
            let _span = operation.measure_thread().iterations(4);
            register_fake_allocation(800, 8);
        }
        // Registered but never measured, so it stays at zero iterations and must
        // be skipped rather than written.
        let _unmeasured = session.operation("unmeasured");

        let directory = tempfile::tempdir().unwrap();
        session.to_report().write_to_directory(directory.path());

        assert!(directory.path().join("measured.json").exists());
        assert!(!directory.path().join("unmeasured.json").exists());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn overwrites_existing_files() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("allocate_vec.json");
        fs::write(&file, "stale contents").unwrap();

        let session = session_with_recorded_work("allocate_vec");
        session.to_report().write_to_directory(directory.path());

        // Parsing succeeds only if the stale, non-JSON contents were replaced.
        let value = read_json(&file);
        assert_eq!(
            value.get("operation").and_then(Value::as_str),
            Some("allocate_vec")
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    #[should_panic(expected = "failed to create benchmark output directory")]
    fn panics_when_output_directory_cannot_be_created() {
        let session = session_with_recorded_work("allocate_vec");
        let directory = tempfile::tempdir().unwrap();

        // A regular file where a directory component is expected makes the
        // recursive directory creation fail.
        let blocker = directory.path().join("blocker");
        fs::write(&blocker, "not a directory").unwrap();

        session
            .to_report()
            .write_to_directory(blocker.join("nested"));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    #[should_panic(expected = "failed to write benchmark output file")]
    fn panics_when_output_file_cannot_be_written() {
        let session = session_with_recorded_work("allocate_vec");
        let directory = tempfile::tempdir().unwrap();

        // A directory occupying the output file's path makes the file write fail.
        fs::create_dir_all(directory.path().join("allocate_vec.json")).unwrap();

        session.to_report().write_to_directory(directory.path());
    }

    #[test]
    #[should_panic(expected = "after sanitization")]
    fn panics_when_operation_names_collide_after_sanitization() {
        let session = Session::new().no_stdout().no_file();

        // Both names sanitize to `group_case.json`, so writing both would silently
        // discard one operation's results.
        for name in ["group/case", "group_case"] {
            let operation = session.operation(name);
            let _span = operation.measure_thread().iterations(4);
            register_fake_allocation(800, 8);
        }

        // The collision is detected before anything is written, so this path is
        // never created.
        session
            .to_report()
            .write_to_directory("collision_is_detected_before_writing");
    }
}
