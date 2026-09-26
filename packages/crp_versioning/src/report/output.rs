// Report publication owns its patch tree and publishes JSON only after all patches.

use std::fs;
use std::path::Path;

use ohno::AppError;

use crate::WriteFileError;

/// Publication operations used by report orchestration, separate from filesystem access.
///
/// Reset invalidates the previous completion marker before replacing the patch tree;
/// completion stages JSON next to its destination before promotion.
/// Ref: docs/implementation.md, "Test boundaries".
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
        if report_path.exists() {
            fs::remove_file(&report_path)
                .map_err(|error| WriteFileError::caused_by(&report_path, error))?;
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
        // Same-directory staging prevents partial JSON from becoming the completion marker.
        let staged_report_path = report_path.with_extension("json.tmp");
        fs::write(&staged_report_path, report.as_bytes())
            .map_err(|error| WriteFileError::caused_by(&staged_report_path, error))?;
        fs::rename(&staged_report_path, &report_path)
            .map_err(|error| WriteFileError::caused_by(&report_path, error))?;
        Ok(())
    }
}
