//! External acquisition for metadata.

use std::fs;
use std::path::Path;

use crp_workspace::metadata::*;
use serde_json::{Value, from_slice};
use tempfile::tempdir;

use crate::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo metadata against real manifests.")]
fn captured_metadata_uses_the_selected_manifest_without_resolving_dependencies() {
    with_io_test(|| {
        let directory = tempdir().unwrap();
        let member = directory.path().join("selected");
        fs::create_dir(&member).unwrap();
        let manifest = member.join("Cargo.toml");
        fs::write(
            &manifest,
            "[package]\nname='selected'\nversion='2.3.4'\n[workspace]\n[lib]\npath='library.rs'\n",
        )
        .unwrap();
        fs::write(member.join("library.rs"), "").unwrap();

        let bytes = capture_metadata(&manifest).unwrap();
        let metadata: MetadataJson = from_slice(&bytes).unwrap();
        assert_eq!(metadata.packages.len(), 1);
        let package = metadata.packages.first().unwrap();
        assert_eq!(package.name, "selected");
        assert_eq!(package.version, "2.3.4");
        assert_eq!(metadata.workspace_members, [package.id.clone()]);
        assert_eq!(
            Path::new(&package.manifest_path).canonicalize().unwrap(),
            manifest.canonicalize().unwrap()
        );
        let raw: Value = from_slice(&bytes).unwrap();
        assert_eq!(raw.get("resolve"), Some(&Value::Null));
        assert!(!member.join("Cargo.lock").try_exists().unwrap());

        assert!(capture_metadata(&member.join("missing.toml")).is_err());
        fs::write(&manifest, "[invalid TOML").unwrap();
        assert!(capture_metadata(&manifest).is_err());
    });
}

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
