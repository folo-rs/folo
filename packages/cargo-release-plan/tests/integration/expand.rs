//! Resolving a plan's version groups into an explicit per-package plan.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;

use cargo_release_plan::{RunInput, RunOutcome, run};
use serde_json::Value;
use tempfile::tempdir;

use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(miri, ignore = "reads proposal files without acquiring a workspace")]
fn accepted_destinations_reach_plan_validation_in_both_modes() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("plan.json");
    fs::write(&input, "invalid plan, retained").unwrap();
    for (preserve_input, output) in [
        (true, directory.path().join("expanded.json")),
        (false, directory.path().join("expanded.json")),
        (false, input.clone()),
    ] {
        let error = run(&RunInput::Expand {
            plan: input.clone(),
            out: output,
            manifest_path: directory.path().join("unused.toml"),
            preserve_input,
            verbose: false,
        })
        .unwrap_err();
        assert!(error.find_source::<serde_json::Error>().is_some());
        assert_eq!(
            fs::read_to_string(&input).unwrap(),
            "invalid plan, retained"
        );
    }
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(miri, ignore = "protects input aliases before reading artifacts")]
fn protected_expansion_rejects_input_aliases_before_reading() {
    let directory = tempdir().unwrap();
    let input = directory.path().join("plan.json");
    fs::write(&input, "retained").unwrap();
    for output in [input.clone(), directory.path().join("missing/../plan.json")] {
        let error = run(&RunInput::Expand {
            plan: input.clone(),
            out: output,
            manifest_path: directory.path().join("unused.toml"),
            preserve_input: true,
            verbose: false,
        })
        .unwrap_err();
        // Invalid JSON is not reached when the output aliases the input.
        assert!(error.find_source::<serde_json::Error>().is_none());
        assert_eq!(fs::read_to_string(&input).unwrap(), "retained");
    }
}

#[test]
#[cfg_attr(miri, ignore = "expands plans in a real workspace")]
fn protected_expansion_preserves_inputs_and_default_expansion_remains_in_place() {
    let fixture = Fixture::new("");
    write_package(&fixture, "api", "1.0.0", "");
    fixture.commit("package");
    let input = fixture.path().join("proposal.json");
    fs::write(
        &input,
        r#"{"schema_version":4,"increments":[{"name":"api","level":"patch"}]}"#,
    )
    .unwrap();
    let original = fs::read(&input).unwrap();
    for output in [
        input.clone(),
        fixture.path().join("missing/../proposal.json"),
    ] {
        _ = run(&RunInput::Expand {
            plan: input.clone(),
            out: output,
            preserve_input: true,
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap_err();
        assert_eq!(fs::read(&input).unwrap(), original);
    }
    let output = fixture.path().join("expanded/plan.json");
    _ = run(&RunInput::Expand {
        plan: input.clone(),
        out: output.clone(),
        preserve_input: true,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert_eq!(fs::read(&input).unwrap(), original);
    let expanded: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(expanded.pointer("/increments/0/version").unwrap(), "1.0.1");
    _ = run(&RunInput::Expand {
        plan: input.clone(),
        out: input.clone(),
        preserve_input: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let expanded: Value = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    assert_eq!(expanded.get("expanded").unwrap(), true);
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "creates a directory symlink")]
fn protected_expansion_rejects_a_symlinked_final_destination() {
    let fixture = Fixture::new("");
    write_package(&fixture, "api", "1.0.0", "");
    fixture.commit("package");
    let input = fixture.path().join("proposal.json");
    fs::write(&input, r#"{"schema_version":4,"increments":[]}"#).unwrap();
    let original = fs::read(&input).unwrap();
    let alias = fixture.path().join("alias");
    symlink(fixture.path(), &alias).unwrap();
    _ = run(&RunInput::Expand {
        plan: input.clone(),
        out: alias.join("proposal.json"),
        preserve_input: true,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fs::read(input).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "uses Git, Cargo metadata and encoded plan files")]
fn utf8_bom_is_accepted_by_expansion_and_application() {
    let fixture = Fixture::new("");
    write_package(&fixture, "api", "1.0.0", "");
    fixture.commit("package");
    let plan = fixture.path().join("plan.json");
    fs::write(
        &plan,
        concat!(
            "\u{feff}",
            r#"{"schema_version":4,"increments":[{"name":"api","level":"patch"}]}"#
        ),
    )
    .unwrap();
    let out = fixture.path().join("expanded.json");
    _ = run(&RunInput::Expand {
        preserve_input: false,
        plan: plan.clone(),
        out: out.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let expanded: Value = serde_json::from_slice(&fs::read(out).unwrap()).unwrap();
    assert_eq!(expanded.pointer("/increments/0/version").unwrap(), "1.0.1");
    _ = run(&RunInput::Apply {
        plan,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(
        fixture
            .read("packages/api/Cargo.toml")
            .contains("version = \"1.0.1\"")
    );
}

/// Read-only expansion names group effects without resolving dependent releases.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn expansion_names_only_packages_whose_versions_move() {
    let fixture = Fixture::new("");
    write_package(&fixture, "helper", "1.0.0", "");
    write_package(
        &fixture,
        "app",
        "0.1.0",
        r#"
[dependencies]
helper = { path = "../helper", version = "1.0.0" }
"#,
    );
    fixture.commit("seed");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [{ "name": "helper", "level": "patch" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");

    run(&RunInput::Expand {
        preserve_input: false,
        plan: plan_path,
        out: expanded_path.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    // Only the package whose version moves is named.
    let expanded: Value =
        serde_json::from_str(&fs::read_to_string(&expanded_path).unwrap()).unwrap();
    let named: Vec<&str> = expanded
        .get("increments")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .map(|entry| entry.get("name").and_then(Value::as_str).unwrap())
        .collect();
    assert_eq!(named, vec!["helper"]);

    let manifest = fixture.read("packages/app/Cargo.toml");
    assert!(manifest.contains("version = \"1.0.0\""), "{manifest}");
    assert!(!fixture.path().join("Cargo.lock").exists());
}

/// Expansion names every group member without installing an unresolved plan.
///
/// Structural expansion cannot authorize application without captured resolution.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn expansion_names_every_group_member_but_requires_preview_before_application() {
    let fixture = Fixture::new(
        r#"
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

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [
            { "name": "shell", "level": "patch" },
            { "name": "loner", "level": "minor" }
        ] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded/plan.json");

    let outcome = run(&RunInput::Expand {
        preserve_input: false,
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
        .unwrap();
    let versions = expanded_versions(increments);
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
    .unwrap_err();

    let impl_manifest =
        fs::read_to_string(fixture.path().join("packages/shell_impl/Cargo.toml")).unwrap();
    assert!(impl_manifest.contains("version = \"0.1.0\""));
    let root = fs::read_to_string(fixture.manifest()).unwrap();
    assert!(root.contains("version = \"=0.1.0\""));
    assert!(!fixture.path().join("Cargo.lock").exists());
}

/// A helper can directly target and align a group that publishes no package.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_helper_directly_targets_an_all_non_publishable_group() {
    let fixture = Fixture::new("");
    write_package(&fixture, "z-helper", "0.1.0", "\npublish = false\n");
    write_package(
        &fixture,
        "a-helper",
        "0.1.0",
        "\npublish = false\n\n[dependencies]\nz-helper = { path = \"../z-helper\", version = \"=0.1.0\" }\n",
    );
    fixture.commit("helper group");
    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [{ "name": "z-helper", "level": "patch" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");

    run(&RunInput::Expand {
        preserve_input: false,
        plan: plan_path,
        out: expanded_path.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();

    let expanded: Value =
        serde_json::from_str(&fs::read_to_string(&expanded_path).unwrap()).unwrap();
    assert_eq!(
        expanded_versions(
            expanded
                .get("increments")
                .and_then(Value::as_array)
                .unwrap()
        ),
        vec![("a-helper", "0.1.1"), ("z-helper", "0.1.1")]
    );

    for helper in ["a-helper", "z-helper"] {
        let manifest = fixture.read(&format!("packages/{helper}/Cargo.toml"));
        assert!(manifest.contains("version = \"0.1.0\""), "{manifest}");
    }
}

/// A group whose members disagree on an explicit version is rejected.
///
/// Expansion is the only place a planner resolves a group, so a hand-edited
/// expanded plan that breaks group uniformity must not reach manifests.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn expand_rejects_disagreeing_versions_within_one_group() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "shell",
        "0.1.0",
        "\n[dependencies]\nshell_impl = { path = \"../shell_impl\", version = \"=0.1.0\" }\n",
    );
    write_package(&fixture, "shell_impl", "0.1.0", "");
    fixture.commit("grouped packages");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [
            { "name": "shell", "version": "0.2.0" },
            { "name": "shell_impl", "version": "0.3.0" }
        ] }"#,
    )
    .unwrap();

    let error = run(&RunInput::Expand {
        preserve_input: false,
        plan: plan_path,
        out: fixture.path().join("expanded.json"),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .expect_err("members of one group cannot take different versions");
    assert!(error.to_string().contains("shell"), "{error}");
}

/// A patch increment level restores a version group whose members drifted apart.
///
/// `check` fails on an inconsistent group even when no released content
/// changed, so that failure must be recoverable through the same
/// increment-level decision recorded in the plan for every other
/// case. Expansion lifts every member to the highest declared version raised by
/// the decided level, which is what returns the group to one version.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_patch_increment_level_realigns_an_inconsistent_group() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "shell",
        "0.1.0",
        "\n[dependencies]\nshell_impl = { path = \"../shell_impl\", version = \"=0.2.0\" }\n",
    );
    write_package(&fixture, "shell_impl", "0.2.0", "");
    fixture.commit("drifted group");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [{ "name": "shell", "level": "patch" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");
    run(&RunInput::Expand {
        preserve_input: false,
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
        .unwrap();
    assert_eq!(
        expanded_versions(increments),
        vec![("shell", "0.2.1"), ("shell_impl", "0.2.1")]
    );
}

/// An exact target equal to the group's highest version aligns lagging members.
///
/// Naming the highest declared version moves lagging members up to it and leaves
/// the leading member unchanged. The lagging members become pending release
/// because their declared versions advanced.
/// Ref: docs/design.md, "Version groups".
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_exact_target_aligns_a_group_without_advancing_its_leader() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "shell",
        "1.0.0",
        "\n[dependencies]\nshell_impl = { path = \"../shell_impl\", version = \"=1.1.0\" }\n",
    );
    write_package(&fixture, "shell_impl", "1.1.0", "");
    fixture.commit("drifted group");

    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [{ "name": "shell", "version": "1.1.0" }] }"#,
    )
    .unwrap();
    let expanded_path = fixture.path().join("expanded.json");
    run(&RunInput::Expand {
        preserve_input: false,
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
        .unwrap();
    assert_eq!(
        expanded_versions(increments),
        vec![("shell", "1.1.0"), ("shell_impl", "1.1.0")]
    );
}

fn expanded_versions(increments: &[Value]) -> Vec<(&str, &str)> {
    increments
        .iter()
        .map(|entry| {
            let name = entry.get("name").and_then(Value::as_str).unwrap();
            let version = entry.get("version").and_then(Value::as_str).unwrap();
            (name, version)
        })
        .collect()
}
