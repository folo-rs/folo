use std::fs;

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
/// Returns an error whose source chain carries the condition that failed: a
/// [`ReadFileError`] or [`WriteFileError`] when the filesystem access fails, a
/// [`ParseError`](crate::ParseError) when the input is not valid TOML, and an
/// [`UnexpectedVersionTypeError`](crate::UnexpectedVersionTypeError) or
/// [`InvalidVersionError`](crate::InvalidVersionError) when a dependency's version field
/// is malformed.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, AppError> {
    let content = fs::read_to_string(&input.path)
        .map_err(|error| ReadFileError::caused_by(input.path.as_path(), error))?;

    let (rewritten, outcome) = freeze_document(&content, &input.path)?;

    let output_path = input.output.as_ref().unwrap_or(&input.path);
    fs::write(output_path, rewritten)
        .map_err(|error| WriteFileError::caused_by(output_path.as_path(), error))?;

    Ok(outcome)
}
