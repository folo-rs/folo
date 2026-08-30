//! Status verdicts.
//!
//! Covers which released-content and manifest changes make a package pending release,
//! carrying unreleased changes, or already released.

use std::fs;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, report_json, seeded_package};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn increment_early_in_a_branch_with_later_changes_is_pending_release() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    write_package(&fixture, "demo", "0.1.1", "");
    fixture.commit("bump version");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 1; }\n");
    fixture.commit("later content");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
    let report = report_json(&fixture, &base);
    assert!(report.contains("\"status\": \"pending-release\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn content_already_on_base_needs_an_increment() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 2; }\n");
    fixture.commit("content without version bump");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("needs-increment"));
    assert!(message.contains("increment-versions"));
    let out_dir = fixture.path().join("out");
    let outcome = run(&RunInput::Report {
        out_dir: out_dir.clone(),
        base: Some(base),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Report { message } => {
            assert!(message.contains("1 needing an increment"));
        }
        other => panic!("expected report, got {other:?}"),
    }
    assert!(out_dir.join("diffs").join("demo.patch").is_file());
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn executable_bit_alone_needs_an_increment() {
    let fixture = seeded_package();
    // The script has to be released content at the anchor, so it lands in the
    // same commit as the version that anchors the comparison.
    fixture.write("packages/demo/script.sh", "echo hi\n");
    write_package(&fixture, "demo", "0.1.1", "");
    fixture.commit("release with script");
    let base = fixture.sha("HEAD");

    // The bit is set through the index rather than the filesystem, because a
    // Windows checkout has no executable permission to set and turns
    // `core.fileMode` off. Committing without restaging keeps the recorded mode
    // where `core.fileMode` is on and would otherwise read it back off disk.
    fixture.git(&["update-index", "--chmod=+x", "packages/demo/script.sh"]);
    fixture.git(&["commit", "-m", "make script executable"]);

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("needs-increment"));

    // The content is untouched, so the mode headers are the only thing that
    // records the change for a reader of the patch.
    let out_dir = fixture.path().join("out");
    run(&RunInput::Report {
        out_dir: out_dir.clone(),
        base: Some(base),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let patch = fs::read_to_string(out_dir.join("diffs").join("demo.patch")).unwrap();
    assert!(patch.contains("old mode 100644"), "{patch}");
    assert!(patch.contains("new mode 100755"), "{patch}");
    assert!(!patch.contains("@@"), "{patch}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn added_packaged_file_needs_an_increment() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/README.md", "hello\n");
    fixture.commit("add readme");
    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    let report = report_json(&fixture, &base);
    assert!(report.contains("\"added\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn deleted_packaged_file_needs_an_increment() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        r#"include = ["src/**", "extra.md"]"#,
    );
    fixture.write("packages/demo/extra.md", "notes\n");
    fixture.commit("with extra");
    let base = fixture.sha("HEAD");
    fs::remove_file(fixture.path().join("packages/demo/extra.md")).unwrap();
    fixture.commit("delete extra");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    let report = report_json(&fixture, &base);
    assert!(report.contains("\"deleted\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn path_dropped_from_include_needs_an_increment() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        r#"include = ["src/**", "extra.md"]"#,
    );
    fixture.write("packages/demo/extra.md", "notes\n");
    fixture.commit("include extra");
    let base = fixture.sha("HEAD");
    write_package(&fixture, "demo", "0.1.0", r#"include = ["src/**"]"#);
    fixture.write("packages/demo/extra.md", "notes\n");
    fixture.commit("drop extra from include");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn moved_package_directory_is_compared_by_name() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("original dir");
    let base = fixture.sha("HEAD");

    fs::rename(
        fixture.path().join("packages/demo"),
        fixture.path().join("packages/relocated"),
    )
    .unwrap();
    let manifest =
        fs::read_to_string(fixture.path().join("packages/relocated/Cargo.toml")).unwrap();
    fixture.write("packages/relocated/Cargo.toml", &manifest);
    fixture.write(
        "packages/relocated/src/lib.rs",
        "pub fn f() { let _ = 3; }\n",
    );
    fixture.commit("move and edit");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
}

/// A reformatted manifest without a version change has unreleased changes.
///
/// A packaged `Cargo.toml` is released content like any other file, so a
/// consumer would receive the reformatted bytes. Comparing manifests for
/// semantic equivalence instead would let a published package differ from the
/// version it was released under.
/// Ref: docs/design.md, "Released content".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn manifest_reformat_without_version_change_needs_an_increment() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/demo/Cargo.toml",
        r#"[package]
name = "demo"
version = "0.1.0"
edition = "2021"

# reformatted
"#,
    );
    fixture.commit("reformat");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn workspace_inherited_field_change_marks_inheriting_package() {
    let fixture = Fixture::new(
        r#"
[workspace.package]
license = "MIT"
"#,
    );
    write_package(&fixture, "demo", "0.1.0", "license.workspace = true\n");
    fixture.commit("inherit license");
    let base = fixture.sha("HEAD");
    fixture.write_workspace(
        r#"
[workspace.package]
license = "Apache-2.0"
"#,
    );
    fixture.commit("change inherited license");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    let report = report_json(&fixture, &base);
    assert!(report.contains("workspace.package.license"));
    assert!(report.contains("\"source\": \"inherited\""));
    // Inherited values are not released content, so they carry no file diff and
    // the package gets no patch artifact.
    assert!(!fixture.path().join("out/diffs/demo.patch").exists());
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn declared_version_below_the_anchor_is_an_error() {
    let fixture = seeded_package();
    write_package(&fixture, "demo", "0.2.0", "");
    fixture.commit("release 0.2.0");
    let base = fixture.sha("HEAD");
    write_package(&fixture, "demo", "0.1.5", "");
    fixture.commit("downgrade");

    let result = run(&RunInput::Check {
        base: Some(base),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    });
    assert!(
        result.is_err(),
        "a version below the anchor must not classify, got {result:?}"
    );
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn inherited_publish_false_excludes_a_package_from_classification() {
    let fixture = Fixture::new("[workspace.package]\npublish = false\n");
    write_package(&fixture, "demo", "0.1.0", "publish.workspace = true");
    write_package(&fixture, "keeper", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 8; }\n");
    fixture.commit("change the unpublished package");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
    assert!(!message.contains("demo"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn new_package_on_the_branch_is_pending_release() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    write_package(&fixture, "fresh", "0.1.0", "");
    fixture.commit("add package");

    let report = report_json(&fixture, &base);
    assert!(report.contains("\"name\": \"fresh\""));
    assert!(report.contains("\"status\": \"pending-release\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_new_package_still_reports_its_untracked_paths() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    write_package(&fixture, "fresh", "0.1.0", "");
    fixture.commit("add package");
    fixture.write("packages/fresh/src/extra.rs", "pub fn extra() {}\n");

    let report = report_json(&fixture, &base);
    assert!(report.contains("src/extra.rs"), "{report}");
}
