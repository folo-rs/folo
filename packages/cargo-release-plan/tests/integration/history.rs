//! Anchor resolution over real Git history.
//!
//! Covers merges on the first-parent line, truncated clones, and packages that
//! leave and return.

use std::fs;

use cargo_release_plan::{CheckFormat, RunInput, run};
use tempfile::TempDir;

use crate::fixture::{Fixture, hermetic_git, write_package};
use crate::harness::{check, seeded_package};

/// A merge commit on the base first-parent line is the anchor.
///
/// The anchor walk follows first parents, which makes each merged pull request
/// one step in the base's history. Following the topic commit that carried the
/// increment instead would anchor on a revision the base line never reached.
/// Ref: docs/design.md, "Anchors across merges".
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
    let mut command = hermetic_git();
    command.args([
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
        base: Some("HEAD".to_string()),
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

/// A package the baseline does not publish is treated as entirely new.
///
/// Whether the name was published under an earlier incarnation and later
/// withdrawn is deliberately not reconsidered. Reconciling a restored name is
/// left to whoever restores it, because guessing which older release a
/// restored directory continues would rest the verdict on commits a shallow
/// fetch need not carry at all.
/// Ref: docs/design.md, "Packages the baseline does not publish".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_package_the_baseline_lacks_is_new_however_it_got_there() {
    let fixture = Fixture::new("");
    write_package(&fixture, "keeper", "0.1.0", "");
    write_package(&fixture, "demo", "0.3.0", "");
    fixture.commit("seed");

    fs::remove_dir_all(fixture.path().join("packages/demo")).unwrap();
    fixture.commit("delete demo");
    let base = fixture.sha("HEAD");

    write_package(&fixture, "demo", "0.3.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("restore demo at the version it carried before");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

/// A package withdrawn and restored keeps the anchor it had before withdrawal.
///
/// The alternative - reading a withdrawn commit as an absence, the way a
/// deleted package is read - would make the restoring commit look like a
/// creation, so everything released before the withdrawal would stop
/// constraining the version and content committed since would silently pass.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_withdrawn_package_is_still_anchored_to_its_last_release() {
    let fixture = Fixture::new("");
    write_package(&fixture, "keeper", "0.1.0", "");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");

    write_package(&fixture, "demo", "0.1.0", "publish = false");
    fixture.commit("withdraw demo from publication");

    write_package(&fixture, "demo", "0.1.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 9; }\n");
    fixture.commit("restore demo with changed content at the same version");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("demo: needs-increment"), "{message}");

    write_package(&fixture, "demo", "0.2.0", "");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

/// Merging the baseline into a branch does not move the anchor.
///
/// The walk runs on the baseline's own first-parent line, so a merge that only
/// appears in the branch is invisible to it. The merge does change the work
/// tree, which is the point: the branch then differs from the baseline by its
/// own changes alone.
/// Ref: docs/design.md, "Anchors across merges".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn merging_the_baseline_into_the_branch_keeps_the_verdict() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");

    fixture.git(&["checkout", "-b", "feature"]);
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 1; }\n");
    fixture.commit("branch content");

    fixture.git(&["checkout", "main"]);
    write_package(&fixture, "demo", "0.2.0", "");
    fixture.commit("release 0.2.0");
    let base = fixture.sha("HEAD");

    fixture.git(&["checkout", "feature"]);
    fixture.git(&["merge", "--no-ff", "-m", "merge main", "main"]);

    // The branch now declares the version the baseline released, so its own
    // content edit is unreleased.
    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("demo: needs-increment"), "{message}");

    write_package(&fixture, "demo", "0.3.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 1; }\n");
    fixture.commit("increment past the baseline release");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}
