use std::path::Path;
use std::{fs, io};

use ohno::AppError;

use crate::freeze::freeze_document;
use crate::{ReadFileError, RunInput, RunOutcome, WriteFileError};

/// Core entry point of the tool, extracted for direct testability.
///
/// Reads the Cargo.toml file at `input.path`, freezes every floating dependency version
/// requirement, and writes the result either back to the input path (when `input.output`
/// is `None`) or to the explicit output path.
///
/// # Errors
///
/// Returns an application error if filesystem access, manifest parsing, or
/// dependency-version validation fails. Underlying causes remain attached for
/// diagnostics.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, AppError> {
    let content = read_file(&input.path, |path| fs::read_to_string(path))?;

    let (rewritten, outcome) = freeze_document(&content, &input.path)?;

    let output_path = input.output.as_ref().unwrap_or(&input.path);
    write_file(output_path, &rewritten, |path, content| {
        fs::write(path, content)
    })?;

    Ok(outcome)
}

fn read_file(
    path: &Path,
    read: impl FnOnce(&Path) -> io::Result<String>,
) -> Result<String, AppError> {
    read(path).map_err(|error| ReadFileError::caused_by(path, error).into())
}

fn write_file(
    path: &Path,
    content: &str,
    write: impl FnOnce(&Path, &str) -> io::Result<()>,
) -> Result<(), AppError> {
    write(path, content).map_err(|error| WriteFileError::caused_by(path, error).into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn read_failure_maps_to_read_file_error_with_source() {
        let source = io::Error::new(io::ErrorKind::NotFound, "missing");
        let error = read_file(Path::new("Cargo.toml"), |_| Err(source)).unwrap_err();

        assert!(error.find_source::<ReadFileError>().is_some());
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn write_failure_maps_to_write_file_error_with_source() {
        let source = io::Error::new(io::ErrorKind::PermissionDenied, "denied");
        let error = write_file(Path::new("Cargo.toml"), "", |_, _| Err(source)).unwrap_err();

        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
    }
}
