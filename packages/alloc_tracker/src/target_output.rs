//! Machine-readable JSON output of memory allocation statistics.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{Report, Session};

/// Subdirectory of the Cargo target directory that receives the JSON files.
const OUTPUT_SUBDIRECTORY: &str = "alloc_tracker";

/// Machine-readable allocation statistics for a single operation.
#[derive(Serialize)]
struct OperationOutput<'a> {
    operation: &'a str,
    total_iterations: u64,
    total_bytes_allocated: u64,
    total_allocations_count: u64,
    mean_bytes_per_iteration: u64,
    mean_allocations_per_iteration: u64,
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
    /// # Examples
    ///
    /// ```no_run
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// {
    ///     let operation = session.operation("work");
    ///     let _span = operation.measure_process();
    ///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    /// }
    ///
    /// session.to_report().write_to_target();
    /// ```
    pub fn write_to_target(&self) {
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
    /// # Examples
    ///
    /// ```
    /// use alloc_tracker::{Allocator, Session};
    ///
    /// #[global_allocator]
    /// static ALLOCATOR: Allocator<std::alloc::System> = Allocator::system();
    ///
    /// let session = Session::new();
    /// {
    ///     let operation = session.operation("work");
    ///     let _span = operation.measure_process();
    ///     let _data = vec![1, 2, 3, 4, 5]; // This allocates memory
    /// }
    ///
    /// let directory = std::env::temp_dir().join("alloc_tracker_example");
    /// session.to_report().write_to_directory(&directory);
    /// ```
    pub fn write_to_directory(&self, directory: impl AsRef<Path>) {
        if self.is_empty() {
            return;
        }

        let directory = directory.as_ref();
        fs::create_dir_all(directory).unwrap_or_else(|error| {
            panic!(
                "failed to create benchmark output directory {}: {error}",
                directory.display()
            )
        });

        for (name, operation) in self.operations() {
            if operation.total_iterations() == 0 {
                continue;
            }

            let output = OperationOutput {
                operation: name,
                total_iterations: operation.total_iterations(),
                total_bytes_allocated: operation.total_bytes_allocated(),
                total_allocations_count: operation.total_allocations_count(),
                mean_bytes_per_iteration: operation.mean_bytes(),
                mean_allocations_per_iteration: operation.mean_allocations(),
            };

            let json = serde_json::to_string_pretty(&output)
                .expect("serializing fixed primitive fields to JSON cannot fail");

            let file_name = format!("{}.json", folo_utils::sanitize_file_name(name));
            let path = directory.join(file_name);
            fs::write(&path, json).unwrap_or_else(|error| {
                panic!(
                    "failed to write benchmark output file {}: {error}",
                    path.display()
                )
            });
        }
    }
}

impl Session {
    /// Writes machine-readable JSON statistics into the Cargo target directory.
    ///
    /// This is a convenience method equivalent to
    /// `self.to_report().write_to_target()`. See
    /// [`Report::write_to_target`](crate::Report::write_to_target) for details.
    ///
    /// # Panics
    ///
    /// Panics if the output directory cannot be created or a file cannot be
    /// written.
    pub fn write_to_target(&self) {
        self.to_report().write_to_target();
    }

    /// Writes machine-readable JSON statistics into the given directory.
    ///
    /// This is a convenience method equivalent to
    /// `self.to_report().write_to_directory(directory)`. See
    /// [`Report::write_to_directory`](crate::Report::write_to_directory) for
    /// details.
    ///
    /// # Panics
    ///
    /// Panics if the output directory cannot be created or a file cannot be
    /// written.
    pub fn write_to_directory(&self, directory: impl AsRef<Path>) {
        self.to_report().write_to_directory(directory);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::fs;

    use crate::Session;
    use crate::allocator::register_fake_allocation;

    fn session_with_recorded_work(name: &str) -> Session {
        let session = Session::new();
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

        session.write_to_directory(directory.path());

        let file = directory.path().join("allocate_vec.json");
        let contents = fs::read_to_string(&file).unwrap();

        assert!(contents.contains("\"operation\": \"allocate_vec\""));
        assert!(contents.contains("\"total_iterations\": 4"));
        assert!(contents.contains("\"total_bytes_allocated\": 800"));
        assert!(contents.contains("\"total_allocations_count\": 8"));
        assert!(contents.contains("\"mean_bytes_per_iteration\": 200"));
        assert!(contents.contains("\"mean_allocations_per_iteration\": 2"));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn sanitizes_operation_name_in_file_name() {
        let session = session_with_recorded_work("group/case name");
        let directory = tempfile::tempdir().unwrap();

        session.write_to_directory(directory.path());

        let file = directory.path().join("group_case_name.json");
        assert!(file.exists());

        let contents = fs::read_to_string(&file).unwrap();
        // The original, unsanitized name is preserved inside the file.
        assert!(contents.contains("\"operation\": \"group/case name\""));
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn empty_session_writes_no_files() {
        let session = Session::new();
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("nested");

        session.write_to_directory(&target);

        // Nothing is written, so the directory is not even created.
        assert!(!target.exists());
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Writes files, which is not supported under Miri isolation.
    fn skips_operations_without_iterations() {
        let session = Session::new();
        {
            let operation = session.operation("measured");
            let _span = operation.measure_thread().iterations(4);
            register_fake_allocation(800, 8);
        }
        // Registered but never measured, so it stays at zero iterations and must
        // be skipped rather than written.
        let _unmeasured = session.operation("unmeasured");

        let directory = tempfile::tempdir().unwrap();
        session.write_to_directory(directory.path());

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
        session.write_to_directory(directory.path());

        let contents = fs::read_to_string(&file).unwrap();
        assert!(!contents.contains("stale"));
        assert!(contents.contains("allocate_vec"));
    }
}
