//! Resolving a plan's version groups into an explicit per-package plan.

use std::fs;

use cargo_release_plan::{RunInput, RunOutcome, run};
use serde_json::Value;

use crate::fixture::{Fixture, write_package};
use crate::harness::check;

/// Expansion names group members the plan did not, and the result applies.
///
/// A planner presents the expanded document for approval, so it must list every
/// package the plan moves and must itself be applicable: the reviewed document
/// is the one that gets applied.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn expand_names_every_group_member_and_the_result_applies() {
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
    write_package(&fixture, "loner", "1.2.3", "");
    fixture.commit("grouped packages");
    let base = fixture.sha("HEAD");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [
            { "name": "shell", "level": "patch" },
            { "name": "loner", "level": "minor" }
        ] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded/plan.json");

    let outcome = run(&RunInput::Expand {
        plan: plan_path,
        out: expanded_path.clone(),
        manifest_path: fixture.manifest(),
        verbose: true,
    })
    .unwrap();
    assert!(matches!(outcome, RunOutcome::Expand { .. }));

    let expanded: Value =
        serde_json::from_str(&fs::read_to_string(&expanded_path).unwrap()).unwrap();
    let increments = expanded
        .get("increments")
        .and_then(Value::as_array)
        .expect("an expanded plan always carries an increments array");
    let versions: Vec<(&str, &str)> = increments
        .iter()
        .map(|entry| {
            (
                entry
                    .get("name")
                    .and_then(Value::as_str)
                    .expect("every expanded increment names a package"),
                entry
                    .get("version")
                    .and_then(Value::as_str)
                    .expect("every expanded increment carries an explicit version"),
            )
        })
        .collect();
    // `shell_impl` was never named, but shares a group with `shell`.
    assert_eq!(
        versions,
        vec![
            ("loner", "1.3.0"),
            ("shell", "0.1.1"),
            ("shell_impl", "0.1.1"),
        ]
    );
    // Levels are already resolved, so nothing is left to decide at apply time.
    assert!(increments.iter().all(|entry| entry.get("level").is_none()));

    run(&RunInput::Apply {
        plan: expanded_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let impl_manifest =
        fs::read_to_string(fixture.path().join("packages/shell_impl/Cargo.toml")).unwrap();
    assert!(impl_manifest.contains("version = \"0.1.1\""));
    let root = fs::read_to_string(fixture.manifest()).unwrap();
    assert!(root.contains("version = \"=0.1.1\""));

    fixture.commit("apply expanded plan");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

/// A group whose members disagree on an explicit version is rejected.
///
/// Expansion is the only place a planner resolves a group, so a hand-edited
/// expanded plan that breaks group uniformity must not reach manifests.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn expand_rejects_disagreeing_versions_within_one_group() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["shell", "shell_impl"]
"#,
    );
    write_package(&fixture, "shell", "0.1.0", "");
    write_package(&fixture, "shell_impl", "0.1.0", "");
    fixture.commit("grouped packages");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [
            { "name": "shell", "version": "0.2.0" },
            { "name": "shell_impl", "version": "0.3.0" }
        ] }"#,
    )
    .unwrap();

    let error = run(&RunInput::Expand {
        plan: plan_path,
        out: fixture.path().join("expanded.json"),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .expect_err("members of one group cannot take different versions");
    assert!(error.to_string().contains('g'), "{error}");
}

/// A change level restores a version group whose members drifted apart.
///
/// `check` fails on an inconsistent group even when no released content
/// changed, so that failure must be recoverable through the same
/// level-based decision the increment-versions skill records for every other
/// case. Expansion lifts every member to the highest declared version raised by
/// the decided level, which is what returns the group to one version.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_level_realigns_an_inconsistent_group() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["shell", "shell_impl"]
"#,
    );
    // Members declaring different versions are what makes the group inconsistent.
    write_package(&fixture, "shell", "0.1.0", "");
    write_package(&fixture, "shell_impl", "0.2.0", "");
    fixture.commit("drifted group");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "a drifted group must fail the check: {message}");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "shell", "level": "patch" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");
    run(&RunInput::Expand {
        plan: plan_path,
        out: expanded_path.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let expanded: Value =
        serde_json::from_str(&fs::read_to_string(&expanded_path).unwrap()).unwrap();
    let increments = expanded
        .get("increments")
        .and_then(Value::as_array)
        .expect("an expanded plan always carries an increments array");
    // Both members land on the highest declared version raised by the level.
    assert!(
        increments
            .iter()
            .all(|entry| entry.get("version") == Some(&Value::String("0.2.1".to_string()))),
        "{increments:?}"
    );

    run(&RunInput::Apply {
        plan: expanded_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.commit("realign group");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

/// An exact target equal to the group's highest version aligns without releasing.
///
/// A group that drifted apart without any member changing has to agree on a
/// version, not receive a new one. Naming the highest declared version lifts the
/// lagging members onto it and leaves the leading member untouched, so no
/// member is published for a change it did not make.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_exact_target_aligns_a_group_without_advancing_its_leader() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["shell", "shell_impl"]
"#,
    );
    write_package(&fixture, "shell", "1.0.0", "");
    write_package(&fixture, "shell_impl", "1.1.0", "");
    fixture.commit("drifted group");
    let base = fixture.sha("HEAD");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 1, "increments": [{ "name": "g", "version": "1.1.0" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");
    run(&RunInput::Expand {
        plan: plan_path,
        out: expanded_path.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let expanded: Value =
        serde_json::from_str(&fs::read_to_string(&expanded_path).unwrap()).unwrap();
    let increments = expanded
        .get("increments")
        .and_then(Value::as_array)
        .expect("an expanded plan always carries an increments array");
    assert!(
        increments
            .iter()
            .all(|entry| entry.get("version") == Some(&Value::String("1.1.0".to_string()))),
        "{increments:?}"
    );

    run(&RunInput::Apply {
        plan: expanded_path,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let leader = fs::read_to_string(fixture.path().join("packages/shell_impl/Cargo.toml")).unwrap();
    assert!(leader.contains("version = \"1.1.0\""), "{leader}");
    let laggard = fs::read_to_string(fixture.path().join("packages/shell/Cargo.toml")).unwrap();
    assert!(laggard.contains("version = \"1.1.0\""), "{laggard}");

    fixture.commit("align group");
    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}
