use std::path::{Path, PathBuf};

use crp_workspace::artifact_path::{resolve_path, write_new};
use ohno::AppError;
use serde::Serialize;

use crate::{ReadFileError, WriteFileError};

/// Writes a new phase outcome without replacing earlier attempt evidence.
#[cfg_attr(test, mutants::skip)] // Atomic filesystem promotion is tested by workspace boundaries.
pub(crate) fn write_outcome(path: &Path, outcome: &impl Serialize) -> Result<(), AppError> {
    write_new(path, |file| {
        serde_json::to_writer_pretty(file, outcome)
            .map_err(|error| WriteFileError::caused_by(path, error).into())
    })
}

/// Checks destinations before creating directories or performing any remote work.
#[cfg_attr(test, mutants::skip)] // Resolution observes existing ancestor directories.
pub(crate) fn require_separate_outputs(output: &Path, artifacts: &Path) -> Result<(), AppError> {
    let output = resolve_path(output)?;
    let artifacts = resolve_path(artifacts)?;
    // Compare every ancestor through the resolver so existing filesystem aliases retain their
    // actual identity. A report below an artifact directory is usable; its parent is not.
    for ancestor in artifacts.ancestors() {
        if resolve_path(ancestor)? == output {
            return Err(OverlappingDestinations::new(output, artifacts).into());
        }
    }
    Ok(())
}

#[cfg_attr(test, mutants::skip)] // Destination inspection is a filesystem boundary.
pub(crate) fn require_new(path: &Path) -> Result<(), AppError> {
    if path
        .try_exists()
        .map_err(|error| ReadFileError::caused_by(path, error))?
    {
        return Err(DestinationOccupied::new(path).into());
    }
    Ok(())
}

/// An existing artifact cannot be replaced by different intent or another attempt.
#[ohno::error]
#[display("publication destination is already occupied: {}", path.display())]
pub(crate) struct DestinationOccupied {
    path: PathBuf,
}

#[ohno::error]
#[display("outcome destination {} overlaps artifact directory {}", output.display(), artifacts.display())]
struct OverlappingDestinations {
    output: PathBuf,
    artifacts: PathBuf,
}
