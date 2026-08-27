//! End-to-end classification and apply tests against hermetic Git fixtures.
//!
//! Each test drives [`cargo_release_plan::run`] directly. Git configuration is
//! pinned by [`common::Fixture`] so tests do not depend on host or user
//! settings. Integer literals assigned to unused locals in generated Rust
//! sources are arbitrary byte-change markers.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};
use tempfile::TempDir;

use crate::common::{Fixture, write_package};

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
fn content_already_on_base_has_unreleased_changes() {
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
fn group_closure_with_member_absent_from_base_is_consistent() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    fixture.commit("alpha only");
    let base = fixture.sha("HEAD");
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("add group member absent from base");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_rewrites_exact_pins_and_expands_groups() {
    let fixture = Fixture::new(
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
fn deleted_packaged_file_has_unreleased_changes() {
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
fn path_dropped_from_include_has_unreleased_changes() {
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
fn manifest_reformat_without_version_change_has_unreleased_changes() {
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
fn inconsistent_group_fails_check_even_when_content_is_unchanged() {
    let fixture = Fixture::new(
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
fn added_packaged_file_has_unreleased_changes() {
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
    let fixture = Fixture::new(
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
        base,
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
fn nested_package_content_belongs_only_to_the_nested_package() {
    let fixture = nested_workspace();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/outer/inner/src/lib.rs",
        "pub fn g() { let _ = 1; }\n",
    );
    fixture.commit("change inner");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    // `inner` is a member only because `outer` depends on it by path, so it must
    // be reconstructed at the anchor too, or the change would look like a brand
    // new package. Its files belong to `inner` alone, so `outer` stays released.
    assert!(message.contains("inner: unreleased-changes"), "{message}");
    assert!(!message.contains("outer: unreleased-changes"), "{message}");
}

/// Workspace whose declared member contains a second package reached by path.
///
/// `packages/*` matches `packages/outer` only, so `inner` is a member solely
/// through the path dependency, and it sits inside the outer package directory.
fn nested_workspace() -> Fixture {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "outer",
        "0.1.0",
        "\n[dependencies]\ninner = { path = \"inner\", version = \"0.1.0\" }",
    );
    fixture.write(
        "packages/outer/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("packages/outer/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    fixture
}

fn seeded_package() -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    fixture
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

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn nested_package_boundary_holds_when_the_workspace_is_below_the_repository_root() {
    let fixture = Fixture::new("");
    fixture.write(
        "sub/Cargo.toml",
        "[workspace]\nmembers = [\"packages/*\"]\nresolver = \"2\"\n",
    );
    fixture.write(
        "sub/packages/outer/Cargo.toml",
        concat!(
            "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[dependencies]\ninner = { path = \"inner\", version = \"0.1.0\" }\n"
        ),
    );
    fixture.write("sub/packages/outer/src/lib.rs", "pub fn f() {}\n");
    fixture.write(
        "sub/packages/outer/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("sub/packages/outer/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed nested workspace");
    let base = fixture.sha("HEAD");

    fixture.write(
        "sub/packages/outer/inner/src/lib.rs",
        "pub fn g() { let _ = 6; }\n",
    );
    fixture.commit("change inner");

    // The workspace root is below the repository root, so member directories and
    // package directories both have to be expressed relative to the repository
    // before the nested-package boundary is applied.
    let (passed, message) = check_workspace(&base, fixture.path().join("sub").join("Cargo.toml"));
    assert!(!passed, "{message}");
    assert!(message.contains("inner: unreleased-changes"), "{message}");
    assert!(!message.contains("outer: unreleased-changes"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn nested_package_boundary_holds_for_a_crate_that_is_not_a_member() {
    let fixture = Fixture::new("exclude = [\"packages/demo/fixture\"]");
    write_package(&fixture, "demo", "0.1.0", "");
    // Excluded from the workspace and depended on by nobody, so it appears in no
    // member list, yet Cargo still stops packing `demo` at its manifest.
    fixture.write(
        "packages/demo/fixture/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("packages/demo/fixture/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    fixture.write(
        "packages/demo/fixture/src/lib.rs",
        "pub fn g() { let _ = 7; }\n",
    );
    fixture.commit("change the excluded nested crate");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_rewrites_pins_declared_by_a_non_publishable_member() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    write_package(
        &fixture,
        "tool",
        "0.1.0",
        concat!(
            "publish = false\n\n",
            "[dependencies]\ndemo = { version = \"=0.1.0\", path = \"../demo\" }\n"
        ),
    );
    fixture.commit("seed");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "demo", "level": "minor" }] }"#,
    )
    .unwrap();

    run(&RunInput::Apply {
        plan: plan_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    // `tool` is never released, but its pin still has to follow demo or the
    // workspace lockfile can no longer be resolved.
    let tool = fs::read_to_string(fixture.path().join("packages/tool/Cargo.toml")).unwrap();
    assert!(tool.contains("version = \"=0.2.0\""), "{tool}");
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
fn verify_packaging_agrees_with_cargo_on_a_clean_tree() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(!message.contains("packaging rule mismatch"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_warns_without_failing_when_cargo_would_pack_an_untracked_file() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    // Untracked files are advisory only, so they are never released content —
    // but Cargo would pack this one, and naming that divergence is the whole
    // job of the cross-check.
    fixture.write("packages/demo/src/extra.rs", "pub fn g() {}\n");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    // The cross-check is advisory, so a mismatch must not change the verdict.
    assert!(passed, "{message}");
    assert!(
        message.contains("packaging rule mismatch for demo"),
        "{message}"
    );
    assert!(
        message.contains("only in `cargo package --list`: src/extra.rs"),
        "{message}"
    );
    assert!(message.contains("only in tool: nothing"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_records_group_verdicts() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let report = report_json(&fixture, &base);

    assert!(report.contains("\"consistent\": true"), "{report}");
    assert!(report.contains("\"alpha\""), "{report}");
    assert!(report.contains("\"beta\""), "{report}");
    assert!(report.contains("\"version\": \"0.1.0\""), "{report}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_rewrites_a_pin_under_a_target_specific_dependency_table() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    write_package(
        &fixture,
        "user",
        "0.1.0",
        "\n[target.'cfg(windows)'.dependencies]\ndemo = { version = \"=0.1.0\", path = \"../demo\" }\n",
    );
    fixture.commit("seed");

    apply_increment(&fixture, "demo", "minor");

    // A pin buried under a target table still has to follow the package it
    // pins, or the workspace stops resolving on that target alone.
    let user = fs::read_to_string(fixture.path().join("packages/user/Cargo.toml")).unwrap();
    assert!(user.contains("version = \"=0.2.0\""), "{user}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_refreshes_the_workspace_lockfile() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("seed");
    let before = fs::read_to_string(fixture.path().join("Cargo.lock")).unwrap();
    assert!(before.contains("0.1.0"), "{before}");

    apply_increment(&fixture, "demo", "minor");

    // `--locked` builds read the lockfile, so leaving the old version there
    // would fail every build that apply was supposed to unblock.
    let after = fs::read_to_string(fixture.path().join("Cargo.lock")).unwrap();
    assert!(after.contains("0.2.0"), "{after}");
}

/// A report directory is reused across runs, so a diff left over from a package
/// that no longer has one would still be read as current.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_replaces_the_diffs_of_an_earlier_run() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    let out_dir = fixture.path().join("out");
    report_json(&fixture, &base);
    let stale = out_dir.join("diffs").join("stale.diff");
    fs::write(&stale, "leftover").unwrap();

    report_json(&fixture, &base);

    assert!(!stale.exists());
    assert!(out_dir.join("report.json").exists());
}

/// A plan that expands to nothing is a no-op, not an error: rewriting no
/// manifests means there is no lockfile drift to refresh either.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn apply_with_an_empty_plan_changes_nothing() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("seed");
    let manifest = fixture.path().join("packages/demo/Cargo.toml");
    let before = fs::read_to_string(&manifest).unwrap();
    let plan_path = fixture.path().join("plan.json");
    fs::write(&plan_path, r#"{ "schema_version": 1, "increments": [] }"#).unwrap();

    run(&RunInput::Apply {
        plan: plan_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: true,
    })
    .unwrap();

    assert_eq!(fs::read_to_string(&manifest).unwrap(), before);
}

/// The packaging probe cross-checks the relevance rules against Cargo itself,
/// so a package Cargo refuses to enumerate must degrade to a warning rather
/// than turn a passing check into a failure.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_warns_when_cargo_cannot_enumerate_a_package() {
    let fixture = Fixture::new("");
    // Cargo reads the manifest for `metadata` but only resolves the referenced
    // file when it packs, so this fails the probe alone.
    write_package(&fixture, "demo", "0.1.0", "readme = \"missing.md\"");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(message.contains("packaging probe failed"), "{message}");
}

fn apply_increment(fixture: &Fixture, name: &str, level: &str) {
    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        format!(
            r#"{{ "schema_version": 1, "increments": [{{ "name": "{name}", "level": "{level}" }}] }}"#
        ),
    )
    .unwrap();

    run(&RunInput::Apply {
        plan: plan_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
}

fn check_verifying_packaging(fixture: &Fixture, base: &str) -> (bool, String) {
    match run(&RunInput::Check {
        base: base.to_string(),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: true,
        verbose: false,
    }) {
        Ok(RunOutcome::Check { passed, message }) => (passed, message),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => panic!("{error}"),
    }
}

fn check(fixture: &Fixture, base: &str) -> (bool, String) {
    check_workspace(base, fixture.manifest())
}

fn check_workspace(base: &str, manifest_path: PathBuf) -> (bool, String) {
    match run(&RunInput::Check {
        base: base.to_string(),
        manifest_path,
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

mod common;
