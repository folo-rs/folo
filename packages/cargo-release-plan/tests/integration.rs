//! End-to-end classification and apply tests against hermetic Git fixtures.
//!
//! Each test drives [`cargo_release_plan::run`] directly. Git identity, signing,
//! and autogc are pinned by [`common::Fixture`]. Integer literals assigned to
//! unused locals in generated Rust sources are arbitrary byte-change markers.

mod common;

use std::fs;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};

use crate::common::{Fixture, write_package, write_workspace};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn increment_early_in_a_branch_with_later_changes_is_releasing() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    write_package(&fixture, "demo", "0.1.1", "");
    fixture.commit("bump version");
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 1; }\n");
    fixture.commit("later content");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
    let report = report_json(&fixture, &base);
    assert!(report.contains("\"status\": \"releasing\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn unreleased_content_already_on_base_fails_check() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 2; }\n");
    fixture.commit("content without version bump");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("unreleased-changes"));
    assert!(message.contains("increment-versions"));
    let out_dir = fixture.path().join("out");
    let outcome = run(&RunInput::Report {
        out_dir: out_dir.clone(),
        base,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Report { message } => {
            assert!(message.contains("1 with unreleased changes"));
        }
        other => panic!("expected report, got {other:?}"),
    }
    assert!(out_dir.join("diffs").join("demo.patch").is_file());
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn group_closure_with_unpublished_member_is_consistent() {
    let fixture = Fixture::new();
    write_workspace(
        &fixture,
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    fixture.commit("alpha only");
    let base = fixture.sha("HEAD");
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("add unpublished group member");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_rewrites_exact_pins_and_expands_groups() {
    let fixture = Fixture::new();
    write_workspace(
        &fixture,
        r#"
[workspace.metadata.release-plan.groups]
g = ["shell", "shell_impl"]

[workspace.dependencies]
shell_impl = { version = "=0.1.0", path = "packages/shell_impl" }
"#,
    );
    write_package(
        &fixture,
        "shell",
        "0.1.0",
        "
[dependencies]
shell_impl = { workspace = true }
",
    );
    write_package(&fixture, "shell_impl", "0.1.0", "");
    fixture.commit("grouped crates");
    let base = fixture.sha("HEAD");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "shell", "level": "patch" }] }"#,
    )
    .unwrap();

    let outcome = run(&RunInput::Apply {
        plan: plan_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(matches!(outcome, RunOutcome::Apply { .. }));

    let impl_manifest =
        fs::read_to_string(fixture.path().join("packages/shell_impl/Cargo.toml")).unwrap();
    assert!(impl_manifest.contains("version = \"0.1.1\""));
    let root = fs::read_to_string(fixture.manifest()).unwrap();
    assert!(root.contains("version = \"=0.1.1\""));

    fixture.commit("apply versions");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_dry_run_does_not_write() {
    let fixture = seeded_package();
    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "demo", "level": "minor" }] }"#,
    )
    .unwrap();
    let before = fs::read_to_string(fixture.path().join("packages/demo/Cargo.toml")).unwrap();
    let outcome = run(&RunInput::Apply {
        plan: plan_path,
        dry_run: true,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Apply { message } => {
            assert!(message.contains("Dry run"));
            assert!(message.contains("1 manifest"));
        }
        other => panic!("expected apply, got {other:?}"),
    }
    let after = fs::read_to_string(fixture.path().join("packages/demo/Cargo.toml")).unwrap();
    assert_eq!(before, after);
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn deleted_packaged_file_is_unreleased() {
    let fixture = Fixture::new();
    write_workspace(&fixture, "");
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
fn path_dropped_from_include_is_unreleased() {
    let fixture = Fixture::new();
    write_workspace(&fixture, "");
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
    let fixture = Fixture::new();
    write_workspace(&fixture, "");
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

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn workspace_inherited_field_change_marks_inheriting_package() {
    let fixture = Fixture::new();
    write_workspace(
        &fixture,
        r#"
[workspace.package]
license = "MIT"
"#,
    );
    write_package(&fixture, "demo", "0.1.0", "license.workspace = true\n");
    fixture.commit("inherit license");
    let base = fixture.sha("HEAD");
    write_workspace(
        &fixture,
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
    let patch = fs::read_to_string(fixture.path().join("out/diffs/demo.patch")).unwrap();
    assert!(patch.contains("Inherited workspace values changed"));
    assert!(patch.contains("workspace.package.license"));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn manifest_reformat_without_version_change_is_unreleased() {
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
    let clone = tempfile::TempDir::new().unwrap();
    let mut command = std::process::Command::new("git");
    command.args([
        "-c",
        "gc.auto=0",
        "clone",
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
fn inconsistent_group_fails_check_even_when_content_is_unchanged() {
    let fixture = Fixture::new();
    write_workspace(
        &fixture,
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    write_package(&fixture, "beta", "0.2.0", "");
    fixture.commit("mismatched group versions");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("inconsistent") || message.contains("different versions"));
    assert!(message.contains("increment-versions"));

    let outcome = run(&RunInput::Check {
        base,
        manifest_path: fixture.manifest(),
        format: CheckFormat::Github,
        verify_packaging: false,
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Check { passed, message } => {
            assert!(!passed);
            assert!(message.contains("::error"));
        }
        other => panic!("expected check, got {other:?}"),
    }
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn added_packaged_file_is_unreleased() {
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
fn github_format_emits_workflow_annotations() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("content");
    let base = fixture.sha("HEAD");

    let outcome = run(&RunInput::Check {
        base,
        manifest_path: fixture.manifest(),
        format: CheckFormat::Github,
        verify_packaging: false,
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Check { passed, message } => {
            assert!(!passed);
            assert!(message.contains("::error"));
            assert!(message.contains("increment-versions"));
        }
        other => panic!("expected check, got {other:?}"),
    }
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn new_package_on_the_branch_is_releasing() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    write_package(&fixture, "fresh", "0.1.0", "");
    fixture.commit("add package");

    let report = report_json(&fixture, &base);
    assert!(report.contains("\"name\": \"fresh\""));
    assert!(report.contains("\"status\": \"releasing\""));
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_replaces_workspace_inherited_version() {
    let fixture = Fixture::new();
    write_workspace(
        &fixture,
        r#"
[workspace.package]
version = "0.1.0"
"#,
    );
    fixture.write(
        "packages/demo/Cargo.toml",
        r#"[package]
name = "demo"
version.workspace = true
edition = "2021"
"#,
    );
    fixture.write("packages/demo/src/lib.rs", "pub fn f() {}\n");
    fixture.commit("inherit version");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "demo", "level": "patch" }] }"#,
    )
    .unwrap();
    run(&RunInput::Apply {
        plan: plan_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let manifest = fs::read_to_string(fixture.path().join("packages/demo/Cargo.toml")).unwrap();
    assert!(manifest.contains("version = \"0.1.1\""));
    assert!(!manifest.contains("version.workspace"));
}

fn seeded_package() -> Fixture {
    let fixture = Fixture::new();
    write_workspace(&fixture, "");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    fixture
}

fn check(fixture: &Fixture, base: &str) -> (bool, String) {
    match run(&RunInput::Check {
        base: base.to_string(),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    }) {
        Ok(RunOutcome::Check { passed, message }) => (passed, message),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => panic!("{error}"),
    }
}

fn report_json(fixture: &Fixture, base: &str) -> String {
    let out_dir = fixture.path().join("out");
    run(&RunInput::Report {
        out_dir: out_dir.clone(),
        base: base.to_string(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fs::read_to_string(out_dir.join("report.json")).unwrap()
}
