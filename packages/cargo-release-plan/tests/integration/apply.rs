//! Applying an increment plan to manifests and the workspace lockfile.

use std::fs;

use cargo_release_plan::{RunInput, RunOutcome, run};

use crate::fixture::{Fixture, write_package};
use crate::harness::{apply_increment, check, seeded_package};

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
