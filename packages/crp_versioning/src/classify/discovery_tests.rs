//! Released-content acquisition tests without full Cargo workspace classification.

use crp_workspace::git::testing::tree_entry;

use super::*;

#[test]
fn declared_resources_keep_the_first_archive_path_claim() {
    let mut released = HashMap::from([("README.md".to_string(), "pkg/README.md".to_string())]);
    let resources = BTreeMap::from([
        ("README.md".to_string(), "shared/README.md".to_string()),
        ("LICENSE".to_string(), "shared/LICENSE".to_string()),
    ]);
    add_resources(&mut released, resources.iter());
    assert_eq!(
        released,
        HashMap::from([
            ("README.md".to_string(), "pkg/README.md".to_string()),
            ("LICENSE".to_string(), "shared/LICENSE".to_string()),
        ])
    );
}

#[test]
fn only_released_anchor_symlinks_are_rejected() {
    let entries = [
        tree_entry("pkg/link", "120000"),
        tree_entry("pkg/plain", tree_mode(false)),
    ];
    let plain = HashMap::from([("plain".to_string(), "pkg/plain".to_string())]);
    reject_anchor_symlinks("pkg", &entries, &plain).unwrap();
    let released = HashMap::from([("link".to_string(), "pkg/link".to_string())]);
    let error = reject_anchor_symlinks("pkg", &entries, &released).unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
}

#[test]
fn optional_reads_separate_absence_from_failures_at_each_acquisition() {
    let path = Path::new("Cargo.lock");
    for kind in [io::ErrorKind::NotFound, io::ErrorKind::PermissionDenied] {
        let result = read_optional_bytes_with(path, "pkg", "Cargo.lock", Err(kind.into()), || {
            panic!("metadata rejection must precede reading")
        });
        if kind == io::ErrorKind::NotFound {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(result.unwrap_err().find_source::<ReadFileError>().is_some());
        }
        let result =
            read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(false), || Err(kind.into()));
        if kind == io::ErrorKind::NotFound {
            assert_eq!(result.unwrap(), None);
        } else {
            assert!(result.unwrap_err().find_source::<ReadFileError>().is_some());
        }
    }
    let error = read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(true), || {
        panic!("a symbolic link must not be read")
    })
    .unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
    assert_eq!(
        read_optional_bytes_with(path, "pkg", "Cargo.lock", Ok(false), || Ok(vec![])).unwrap(),
        Some(vec![])
    );
    assert_eq!(
        read_optional_bytes_with(
            path,
            "pkg",
            "Cargo.lock",
            Ok(false),
            || Ok(b"lock".to_vec())
        )
        .unwrap(),
        Some(b"lock".to_vec())
    );
}

#[test]
fn filesystem_link_metadata_stops_hash_input_selection() {
    let root = Path::new("repository");
    let released = HashMap::from([("link".to_string(), "pkg/link".to_string())]);
    let error =
        validated_work_tree_files_with(root, "pkg", &released, &WorkTreeModes::default(), |path| {
            assert_eq!(path, root.join("pkg/link"));
            Ok(true)
        })
        .unwrap_err();
    assert!(error.find_source::<SymlinkReleasedError>().is_some());
}
