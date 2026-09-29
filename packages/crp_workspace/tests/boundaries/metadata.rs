//! External acquisition for metadata.

use std::fs;
use std::path::Path;

use crp_workspace::metadata::*;
use tempfile::tempdir;

#[cfg_attr(
    miri,
    ignore = "Manifest argument normalization probes filesystem case behavior."
)]
#[test]
fn cargo_manifest_basename_preserves_canonical_and_empty_paths() {
    for path in ["", "Cargo.toml", "workspace/Cargo.toml"] {
        assert_eq!(cargo_manifest_path(Path::new(path)), Path::new(path));
    }
}

#[cfg_attr(miri, ignore = "Probes real directory entries and filesystem aliases.")]
#[test]
fn cargo_manifest_basename_only_rewrites_a_filesystem_alias() {
    let directory = tempdir().unwrap();
    let manifest = directory.path().join("Cargo.toml");
    fs::write(&manifest, "[workspace]\n").unwrap();
    let alias = directory.path().join("cargo.toml");
    let expected = match fs::read(&alias) {
        Ok(bytes) => {
            assert_eq!(bytes, fs::read(&manifest).unwrap());
            &manifest
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => &alias,
        Err(error) => panic!("filesystem probe failed: {error}"),
    };
    assert_eq!(&cargo_manifest_path(&alias), expected);
    let other = directory.path().join("manifest.input");
    fs::write(&other, "[workspace]\n").unwrap();
    assert_eq!(cargo_manifest_path(&other), other);
}
