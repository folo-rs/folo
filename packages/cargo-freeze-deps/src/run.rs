use std::fs;

use crate::freeze::freeze_document;
use crate::{RunError, RunInput, RunOutcome};

/// Core entry point of the tool, extracted for direct testability.
///
/// Reads the Cargo.toml file at `input.path`, freezes every floating dependency version
/// requirement, and writes the result either back to the input path (when `input.output`
/// is `None`) or to the explicit output path.
///
/// # Errors
///
/// See [`RunError`] for the full taxonomy of failures.
#[doc(hidden)]
pub fn run(input: &RunInput) -> Result<RunOutcome, RunError> {
    let content = fs::read_to_string(&input.path).map_err(RunError::Io)?;

    let (rewritten, outcome) = freeze_document(&content)?;

    let output_path = input.output.as_ref().unwrap_or(&input.path);
    fs::write(output_path, rewritten).map_err(RunError::Io)?;

    Ok(outcome)
}
