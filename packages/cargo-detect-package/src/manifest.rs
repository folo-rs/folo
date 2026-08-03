// Cargo manifest reading and parsing.
//
// Both workspace-root discovery and package detection need the parsed contents of a `Cargo.toml`,
// so the read and parse failure modes are constructed in exactly one place here.

use std::path::Path;

use ohno::AppError;
use toml::Value;

use crate::pal::Filesystem;
use crate::{ParseManifestError, ReadManifestError};

/// Reads and parses the `Cargo.toml` manifest in the given directory.
pub(crate) fn read_manifest(directory: &Path, fs: &impl Filesystem) -> Result<Value, AppError> {
    let contents = fs
        .read_cargo_toml(directory)
        .map_err(|error| ReadManifestError::caused_by(directory.join("Cargo.toml"), error))?;

    let manifest = toml::from_str(&contents)
        .map_err(|error| ParseManifestError::caused_by(directory.join("Cargo.toml"), error))?;

    Ok(manifest)
}
