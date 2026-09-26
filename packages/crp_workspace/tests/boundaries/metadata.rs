//! External acquisition for metadata.

#[cfg(unix)]
use std::collections::BTreeMap;
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

#[cfg(unix)]
#[cfg_attr(miri, ignore)] // Creates a filesystem symbolic link, which Miri cannot emulate.
#[test]
fn member_resolution_follows_filesystem_aliases() {
    use std::os::unix::fs::symlink;

    let root = tempdir().unwrap();
    let member = root.path().join("member");
    fs::create_dir_all(&member).unwrap();
    symlink(&member, root.path().join("alias")).unwrap();
    let members = BTreeMap::from([(member, "member".to_string())]);
    let canonical_members = canonical_members_by_dir(&members);

    assert_eq!(
        resolved_member(root.path(), "alias", &members, &canonical_members),
        Some("member")
    );
}
