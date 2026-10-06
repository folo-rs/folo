// Report publication owns its patch tree and publishes JSON only after all patches.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::Path;

use crp_workspace::artifact_path::write_new;
use ohno::AppError;

use crate::WriteFileError;

/// Publication operations used by report orchestration, separate from filesystem access.
///
/// Reset invalidates the previous completion marker before replacing the patch tree;
/// completion stages JSON next to its destination before promotion.
/// Ref: packages/cargo-release-plan/docs/implementation.md, "Test boundaries".
pub(crate) trait ReportOutput {
    fn reset(&mut self) -> Result<(), AppError>;
    fn write_patch(&mut self, name: &str, patch: &str) -> Result<(), AppError>;
    fn complete(&mut self, report: &str) -> Result<(), AppError>;
}

/// Filesystem adapter for one report directory and its owned patches.
pub(crate) struct FileOutput<'a> {
    pub(crate) directory: &'a Path,
}

// Real filesystem replacement and staging require integration tests. All selection,
// contents and publication sequencing remain in the mutation-tested report core.
#[cfg_attr(test, mutants::skip)]
impl ReportOutput for FileOutput<'_> {
    fn reset(&mut self) -> Result<(), AppError> {
        fs::create_dir_all(self.directory)
            .map_err(|error| WriteFileError::caused_by(self.directory, error))?;
        let report_path = self.directory.join("report.json");
        // Invalidate completion before touching the tool-owned patch subtree.
        match fs::remove_file(&report_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(WriteFileError::caused_by(&report_path, error).into()),
        }
        let diffs_dir = self.directory.join("diffs");
        if diffs_dir.exists() {
            fs::remove_dir_all(&diffs_dir)
                .map_err(|error| WriteFileError::caused_by(&diffs_dir, error))?;
        }
        fs::create_dir_all(&diffs_dir)
            .map_err(|error| WriteFileError::caused_by(&diffs_dir, error))?;
        Ok(())
    }

    fn write_patch(&mut self, name: &str, patch: &str) -> Result<(), AppError> {
        let path = self.directory.join("diffs").join(name);
        fs::write(&path, patch.as_bytes())
            .map_err(|error| WriteFileError::caused_by(&path, error))?;
        Ok(())
    }

    fn complete(&mut self, report: &str) -> Result<(), AppError> {
        let report_path = self.directory.join("report.json");
        // Discard abandoned staging as an entry rather than truncating possibly shared
        // contents. Exclusive same-directory staging protects hard-linked source files.
        let staged_report_path = report_path.with_extension("json.tmp");
        match fs::remove_file(&staged_report_path) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(error) => return Err(WriteFileError::caused_by(&staged_report_path, error).into()),
        }
        write_new(&report_path, |file| {
            file.write_all(report.as_bytes())
                .map_err(|error| WriteFileError::caused_by(&report_path, error).into())
        })
    }
}
