//! Anchor resolution over real Git history: merges on the first-parent
//! line, truncated clones, and packages that leave and return.

use std::fs;
use std::process::Command;

use cargo_release_plan::{CheckFormat, RunInput, run};
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, seeded_package};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn merge_commit_on_base_first_parent_line_is_the_anchor() {
    let fixture = seeded_package();
    fixture.git(&["checkout", "-b", "topic"]);
    write_package(&fixture, "demo", "0.1.1", "");
    fixture.commit("version on topic");
    fixture.git(&["checkout", "main"]);
    fixture.git(&["merge", "--no-ff", "-m", "merge topic", "topic"]);
    let base_after_merge = fixture.sha("HEAD");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 4; }\n");
    fixture.commit("content after merge");

    let (passed, message) = check(&fixture, &base_after_merge);
    assert!(!passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn shallow_history_without_a_version_change_is_an_error() {
    let fixture = seeded_package();
    fixture.write("packages/demo/README.md", "docs\n");
    fixture.commit("second commit at the same version");
    // Same declared version, second commit. A clone that contains only HEAD
    // cannot see the creation commit, so classification must error rather than
    // pass.
    let clone = TempDir::new().unwrap();
    let mut command = Command::new("git");
    command.args([
        "-c",
        "gc.auto=0",
        "clone",
        // The source is a local path, for which Git's default local-clone
        // optimization copies the whole object store and ignores the requested
        // depth. Forcing the regular transport is what makes `--depth` produce
        // an actual shallow boundary here.
        "--no-local",
        "--depth",
        "1",
        "--branch",
        "main",
    ]);
    command.arg(fixture.path());
    command.arg(clone.path());
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let result = run(&RunInput::Check {
        base: "HEAD".to_string(),
        manifest_path: clone.path().join("Cargo.toml"),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    });
    assert!(
        result.is_err(),
        "shallow clone must not pass classification, got {result:?}"
    );
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn reintroduced_package_is_anchored_to_the_version_it_carried_before_deletion() {
    let fixture = Fixture::new("");
    write_package(&fixture, "keeper", "0.1.0", "");
    write_package(&fixture, "demo", "0.3.0", "");
    fixture.commit("seed");

    fs::remove_dir_all(fixture.path().join("packages/demo")).unwrap();
    fixture.commit("delete demo");
    let base = fixture.sha("HEAD");

    write_package(&fixture, "demo", "0.3.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("reintroduce demo at an already-released version");

    // Restoring a package is not creating it: 0.3.0 was already carried by the
    // base line, so content that differs from it is unreleased.
    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("demo: unreleased-changes"), "{message}");

    write_package(&fixture, "demo", "0.4.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("increment past the released version");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn reintroduced_package_is_anchored_to_its_last_version_change() {
    let fixture = Fixture::new("");
    write_package(&fixture, "keeper", "0.1.0", "");
    write_package(&fixture, "demo", "0.2.0", "");
    fixture.commit("seed");

    write_package(&fixture, "demo", "0.3.0", "");
    fixture.commit("release 0.3.0");

    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 8; }\n");
    fixture.commit("content without a version bump");

    fs::remove_dir_all(fixture.path().join("packages/demo")).unwrap();
    fixture.commit("delete demo");
    let base = fixture.sha("HEAD");

    write_package(&fixture, "demo", "0.3.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 8; }\n");
    fixture.commit("restore demo at the same version");

    // The anchor is the commit that introduced 0.3.0, not the newest commit that
    // merely carried the package, so content committed afterwards without an
    // increment is still unreleased after the round trip through deletion.
    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("demo: unreleased-changes"), "{message}");
}
