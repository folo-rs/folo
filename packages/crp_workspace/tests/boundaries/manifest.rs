//! External acquisition for manifest.

use std::fs;
use std::path::Path;

use crp_workspace::manifest::*;

/// Probing a directory without cased names reports sensitive.
///
/// The probe re-opens an existing entry under a flipped spelling, so a directory that offers no
/// flippable entry cannot prove insensitivity and must yield the stricter answer.
#[cfg_attr(miri, ignore)] // Reads a real directory, which Miri cannot emulate.
#[test]
fn probing_a_directory_without_cased_names_reports_sensitive() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("123"), "").unwrap();

    assert_eq!(PathCase::probe(temp.path()), PathCase::Sensitive);
}

#[cfg_attr(miri, ignore)] // Reads a real directory, which Miri cannot emulate.
#[test]
fn probing_an_unreadable_directory_reports_sensitive() {
    let temp = tempfile::tempdir().unwrap();

    assert_eq!(
        PathCase::probe(&temp.path().join("absent")),
        PathCase::Sensitive
    );
}

#[cfg_attr(miri, ignore)] // Reads a real directory, which Miri cannot emulate.
#[test]
fn path_case_probe_agrees_with_the_filesystem() {
    let dir = tempfile::TempDir::new().unwrap();
    fs::write(dir.path().join("Probe.txt"), "x").unwrap();
    let probed = PathCase::probe(dir.path());
    let observed = if dir.path().join("PROBE.TXT").exists() {
        PathCase::Insensitive
    } else {
        PathCase::Sensitive
    };
    assert_eq!(probed, observed);
}

#[cfg_attr(miri, ignore)] // Reads the filesystem, which Miri cannot emulate.
#[test]
fn unreadable_directory_probes_as_case_sensitive() {
    // The stricter answer never widens member matching, so an unreadable
    // directory must not relax it.
    assert_eq!(
        PathCase::probe(Path::new("cargo-release-plan-no-such-directory")),
        PathCase::Sensitive
    );
}
