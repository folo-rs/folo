//! External acquisition for classify.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

#[cfg(unix)]
use crp_impl::SymlinkReleasedError;
use crp_impl::classify::*;
use tempfile::tempdir;

#[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
#[test]
fn read_optional_bytes_missing_is_none() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("gone.txt");
    assert_eq!(read_optional_bytes(&path, "pkg", "gone.txt").unwrap(), None);
}

#[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
#[test]
fn read_optional_bytes_reads_file() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("here.txt");
    fs::write(&path, "hi").unwrap();
    assert_eq!(
        read_optional_bytes(&path, "pkg", "here.txt")
            .unwrap()
            .as_deref(),
        Some(b"hi".as_slice())
    );
}

#[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
#[test]
fn read_optional_bytes_rejects_non_not_found_errors() {
    let dir = tempdir().unwrap();
    let _ = read_optional_bytes(dir.path(), "pkg", "dir").unwrap_err();
}

/// Read optional bytes rejects a symbolic link.
///
/// A link cannot be compared against history, because Cargo would pack the target's bytes while
/// Git stores the target's path.
#[cfg(unix)]
#[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
#[test]
fn read_optional_bytes_rejects_a_symbolic_link() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("real.txt"), "hi").unwrap();
    let link = dir.path().join("link.txt");
    symlink("real.txt", &link).unwrap();

    let error = read_optional_bytes(&link, "pkg", "a/link.txt").unwrap_err();
    let reported = error
        .find_source::<SymlinkReleasedError>()
        .expect("a link is refused with the dedicated condition")
        .to_string();
    assert!(reported.contains("a/link.txt"), "{reported}");
    assert!(reported.contains("pkg"), "{reported}");
}
