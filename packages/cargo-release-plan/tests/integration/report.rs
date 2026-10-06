//! Report and check output: the JSON document and the failure renderings.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};
use serde_json::{Value, json};
use tempfile::tempdir;

use crate::fixture::{Fixture, GLOBAL_CONFIG, write_package};
use crate::harness::{check, report_json, seeded_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "checks report source isolation against real Git and Cargo"
)]
fn report_rejects_source_output_without_resetting_existing_evidence() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/evidence/report.json", "source input");
    fixture.write("packages/demo/src/evidence/diffs/keep", "source subtree");
    let result = run(&RunInput::Report {
        out_dir: fixture.path().join("packages/demo/src/evidence"),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    });
    result.unwrap_err();
    assert_eq!(
        fixture.read("packages/demo/src/evidence/report.json"),
        "source input"
    );
    assert_eq!(
        fixture.read("packages/demo/src/evidence/diffs/keep"),
        "source subtree"
    );
}

#[test]
#[cfg_attr(miri, ignore = "checks report isolation in a linked Git worktree")]
fn report_protects_linked_worktree_and_shared_repository_storage() {
    let fixture = seeded_package();
    let directory = tempdir().unwrap();
    let checkout = directory.path().join("checkout");
    fixture.git(&[
        "worktree",
        "add",
        "--detach",
        checkout.to_str().unwrap(),
        "HEAD",
    ]);
    let pointer = fs::read_to_string(checkout.join(".git")).unwrap();
    let administration = PathBuf::from(pointer.trim().strip_prefix("gitdir: ").unwrap());
    for output in [
        fixture.path().join(".git/evidence"),
        administration.join("evidence"),
    ] {
        let result = run(&RunInput::Report {
            out_dir: output.clone(),
            release_history: Some("HEAD".to_owned()),
            merge_target: None,
            manifest_path: checkout.join("Cargo.toml"),
            verbose: false,
        });
        result.unwrap_err();
        assert!(!output.exists());
    }
    assert_eq!(fixture.sha("HEAD"), fixture.sha("main"));
}

#[test]
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "compares released-input and captured-source symlink selection"
)]
fn report_does_not_assess_ignored_descendant_redirects() {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    fixture.write(".gitignore", "packages/demo/src/generated\n");
    fixture.commit("ignore generated content");
    let link = fixture.path().join("packages/demo/src/generated");
    symlink(external.path(), &link).unwrap();
    fs::write(external.path().join("report.json"), "not a released input").unwrap();
    let status = report_command(&fixture, external.path()).output().unwrap();
    assert!(status.status.success(), "{status:?}");
    let report: Value =
        serde_json::from_slice(&fs::read(external.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report.pointer("/packages/0/status").unwrap(), "unchanged");
    assert_eq!(fs::read_link(&link).unwrap(), external.path());

    // Captured-source commands reject this link rather than assess its descendants.
    let evidence = fixture.path().join("prepared");
    run(&RunInput::Prepare {
        output: evidence.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!evidence.exists());
}

#[test]
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "protects a tracked link entry from native report cleanup"
)]
fn report_preserves_tracked_link_entries_outside_packages() {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    let link = fixture.path().join("evidence/diffs/source-link");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    symlink(external.path(), &link).unwrap();
    fixture.commit("tracked nonpackage link");
    fixture.write("evidence/report.json", "previous completion");
    let status = report_command(&fixture, &fixture.path().join("evidence"))
        .output()
        .unwrap();
    assert!(fs::symlink_metadata(&link).is_ok(), "{status:?}");
    assert_eq!(fs::read_link(link).unwrap(), external.path());
    assert_eq!(fixture.read("evidence/report.json"), "previous completion");
    assert!(!status.status.success());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects the Git index selected by a child environment"
)]
fn report_preserves_external_git_index() {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    let index = external.path().join("report.json");
    fs::copy(fixture.path().join(".git/index"), &index).unwrap();
    let before = fs::read(&index).unwrap();
    let status = report_command(&fixture, external.path())
        .env("GIT_INDEX_FILE", &index)
        .output()
        .unwrap();
    assert_eq!(fs::read(&index).unwrap(), before, "{status:?}");
    assert!(!status.status.success());
    assert!(!external.path().join("diffs").exists());
    let accepted = report_command(&fixture, &external.path().join("disjoint"))
        .env("GIT_INDEX_FILE", &index)
        .output()
        .unwrap();
    assert!(accepted.status.success(), "{accepted:?}");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects Git's effective object store before report cleanup"
)]
fn report_preserves_external_git_objects() {
    assert_report_preserves_objects(|_, objects, command| {
        command.env("GIT_OBJECT_DIRECTORY", objects);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects Git's effective alternate store before report cleanup"
)]
fn report_preserves_external_git_alternates() {
    assert_report_preserves_objects(|fixture, objects, _| {
        fixture.write(".git/objects/info/alternates", objects.to_str().unwrap());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects an environment-selected alternate object store"
)]
fn report_preserves_environment_git_alternates() {
    assert_report_preserves_objects(|fixture, objects, command| {
        fs::create_dir_all(fixture.path().join(".git/objects")).unwrap();
        command.env("GIT_ALTERNATE_OBJECT_DIRECTORIES", objects);
    });
}

fn assert_report_preserves_objects(configure: impl Fn(&Fixture, &Path, &mut Command)) {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    let objects = external.path().join("diffs");
    let head = fixture.sha("HEAD");
    fs::rename(fixture.path().join(".git/objects"), &objects).unwrap();
    let (fanout, name) = head.split_at_checked(2).unwrap();
    let object = objects.join(fanout).join(name);
    let before = fs::read(&object).unwrap();
    let mut command = report_command(&fixture, external.path());
    configure(&fixture, &objects, &mut command);
    let status = command.output().unwrap();
    assert!(object.exists(), "{status:?}");
    assert_eq!(fs::read(&object).unwrap(), before);
    assert!(!status.status.success());
    assert!(!external.path().join("report.json").exists());

    let mut command = report_command(&fixture, &external.path().join("accepted"));
    configure(&fixture, &objects, &mut command);
    let status = command.output().unwrap();
    assert!(status.status.success(), "{status:?}");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects Git's configured hooks location before report cleanup"
)]
fn report_preserves_external_git_hooks() {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    let hooks = external.path().join("diffs");
    fs::create_dir_all(&hooks).unwrap();
    fs::write(hooks.join("pre-commit"), "hook configuration").unwrap();
    fixture.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    let status = report_command(&fixture, external.path()).output().unwrap();
    assert!(hooks.join("pre-commit").exists(), "{status:?}");
    assert_eq!(
        fs::read(hooks.join("pre-commit")).unwrap(),
        b"hook configuration"
    );
    assert!(!status.status.success());
    assert!(!external.path().join("report.json").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects Git input aliases with native symlinks or junctions"
)]
fn report_preserves_entries_used_to_reach_effective_git_inputs() {
    for kind in [
        "index",
        "objects",
        "hooks",
        "alternate",
        "environment-alternate",
    ] {
        let fixture = seeded_package();
        let external = tempdir().unwrap();
        let output = fixture.path().join("evidence");
        let link = output.join("diffs/link");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        directory_alias(external.path(), &link);
        fs::copy(
            fixture.path().join(".git/index"),
            external.path().join("index"),
        )
        .unwrap();
        let object_name = if cfg!(windows) {
            "objects;quoted"
        } else {
            "objects:quoted"
        };
        fs::rename(
            fixture.path().join(".git/objects"),
            external.path().join(object_name),
        )
        .unwrap();
        fs::create_dir_all(fixture.path().join(".git/objects")).unwrap();
        let relative = format!("evidence/diffs/link/{object_name}");
        if kind == "alternate" {
            // Git resolves file alternates relative to the primary object directory.
            fixture.write(
                ".git/objects/info/alternates",
                &format!("\"../../{relative}\"\n"),
            );
            // A cycle is handled by Git's finite store set, not our own traversal.
            fs::create_dir_all(external.path().join(object_name).join("info")).unwrap();
            fs::write(
                external.path().join(object_name).join("info/alternates"),
                fixture.path().join(".git/objects").to_str().unwrap(),
            )
            .unwrap();
        }
        if kind == "hooks" {
            fixture.git(&["config", "core.hooksPath", "evidence/diffs/link/hooks"]);
        }
        fixture.write("evidence/report.json", "previous report");
        for (destination, accepted) in [(&output, false), (&fixture.path().join("accepted"), true)]
        {
            let mut command = report_command(&fixture, destination);
            match kind {
                "alternate" => {}
                "environment-alternate" => {
                    command.env(
                        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                        format!("\"{relative}\""),
                    );
                }
                _ => {
                    command.env("GIT_OBJECT_DIRECTORY", external.path().join(object_name));
                }
            }
            if kind == "objects" {
                command.env("GIT_OBJECT_DIRECTORY", &relative);
            } else if kind == "index" {
                command.env("GIT_INDEX_FILE", "evidence/diffs/link/index");
            }
            let status = command.output().unwrap();
            assert_eq!(status.status.success(), accepted, "{kind}: {status:?}");
            fs::symlink_metadata(&link).unwrap();
            assert_eq!(fixture.read("evidence/report.json"), "previous report");
        }
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects active included Git configuration before report reset"
)]
fn report_preserves_external_git_configuration() {
    for (key, contents) in [
        ("include.path", "[core]\n\tignorecase = false\n"),
        ("core.attributesFile", "# configured attributes\n"),
        ("core.excludesFile", "# configured excludes\n"),
    ] {
        let fixture = seeded_package();
        let external = tempdir().unwrap();
        let config = external.path().join("report.json");
        fs::write(&config, contents).unwrap();
        fixture.git(&["config", key, config.to_str().unwrap()]);
        let status = report_command(&fixture, external.path()).output().unwrap();
        assert!(!status.status.success(), "{key}: {status:?}");
        assert_eq!(fs::read_to_string(&config).unwrap(), contents);
        assert!(!external.path().join("diffs").exists());
        let accepted = report_command(&fixture, &external.path().join("disjoint"))
            .output()
            .unwrap();
        assert!(accepted.status.success(), "{key}: {accepted:?}");
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "protects an alternate descriptor reached through a symlink"
)]
fn report_preserves_external_alternate_descriptor() {
    let fixture = seeded_package();
    let external = tempdir().unwrap();
    let store = tempdir().unwrap();
    fs::rename(
        fixture.path().join(".git/objects"),
        store.path().join("objects"),
    )
    .unwrap();
    let descriptor = external.path().join("report.json");
    let contents = store.path().join("objects").to_str().unwrap().to_owned();
    fs::write(&descriptor, &contents).unwrap();
    fs::create_dir_all(fixture.path().join(".git/objects/info")).unwrap();
    symlink(
        &descriptor,
        fixture.path().join(".git/objects/info/alternates"),
    )
    .unwrap();
    let status = report_command(&fixture, external.path()).output().unwrap();
    assert!(!status.status.success(), "{status:?}");
    assert_eq!(fs::read_to_string(&descriptor).unwrap(), contents);
}

#[cfg(unix)]
pub(crate) fn directory_alias(source: &Path, alias: &Path) {
    symlink(source, alias).unwrap();
}

#[cfg(windows)]
pub(crate) fn directory_alias(source: &Path, alias: &Path) {
    let status = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-Command",
            "$ErrorActionPreference='Stop'; New-Item -ItemType Junction -Path $env:CRP_TEST_ALIAS -Target $env:CRP_TEST_SOURCE | Out-Null"])
        .env("CRP_TEST_SOURCE", source)
        .env("CRP_TEST_ALIAS", alias)
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
}

pub(crate) fn report_command(fixture: &Fixture, output: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
    command
        .current_dir(fixture.path())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", GLOBAL_CONFIG.path().join("config"))
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .args(["report", "--release-history", "HEAD", "--out-dir"])
        .arg(output);
    command
}

/// A compatible edge remains valid inside a transitively derived group.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn check_accepts_a_compatible_requirement_within_a_transitive_group() {
    let fixture = Fixture::new("");
    write_package(&fixture, "gamma", "1.1.0", "");
    write_package(
        &fixture,
        "beta",
        "1.1.0",
        "\n[dependencies]\ngamma = { path = \"../gamma\", version = \"=1.1.0\" }\n",
    );
    write_package(
        &fixture,
        "alpha",
        "1.1.0",
        "\n[dependencies]\nbeta = { path = \"../beta\", version = \"=1.1.0\" }\ngamma = { path = \"../gamma\", version = \"1.1.0\" }\n",
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
}

/// A stale exact pin still forms a group and is reported as version drift.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn check_reports_a_stale_exact_pin_without_losing_its_group() {
    let fixture = group_fixture("=1.0.0");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(
        message.contains("does not name the version it declares"),
        "{message}"
    );
    let report: Value = serde_json::from_str(&report_json(&fixture, &base)).unwrap();
    assert_eq!(
        report.pointer("/groups/lib/members"),
        Some(&json!(["lib", "lib_impl"]))
    );
}

/// A workspace whose `lib` requires its group sibling `lib_impl` with the given requirement.
fn group_fixture(requirement: &str) -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "lib_impl", "1.1.0", "");
    write_package(
        &fixture,
        "lib",
        "1.1.0",
        &format!(
            r#"
[dependencies]
lib_impl = {{ path = "../lib_impl", version = "{requirement}" }}
"#
        ),
    );
    fixture
}

/// A requirement that does not name its target's declared version fails the check.
///
/// The workspace pins every intra-workspace requirement to the version its target declares, so
/// a requirement that merely admits that version is drift the merge gate has to catch.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn check_rejects_a_requirement_that_does_not_name_the_declared_version() {
    let fixture = Fixture::new("");
    write_package(&fixture, "helper", "1.1.0", "");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        r#"
[dependencies]
helper = { path = "../helper", version = "1.0.0" }
"#,
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(
        message.contains("does not name the version it declares"),
        "{message}"
    );
    assert!(message.contains("1.1.0"), "{message}");
}

/// A package exposing a dependency that breaks must break as well.
///
/// `demo` re-exports `helper` types, declared through its allow-list, so `helper` moving to an
/// incompatible version changes the identity of what `demo` exposes.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn check_rejects_a_public_dependency_breaking_alone() {
    let fixture = public_dependency_fixture("1.0.0", "0.1.0");
    let base = fixture.sha("HEAD");

    // `helper` releases 1.0.0 -> 2.0.0 while `demo` stays on a compatible 0.1.1.
    write_public_dependency_packages(&fixture, "2.0.0", "0.1.1");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(
        message.contains("must release a breaking change of its own"),
        "{message}"
    );
}

/// A workspace whose `demo` exposes `helper` in its public API, at the given versions.
fn public_dependency_fixture(helper: &str, demo: &str) -> Fixture {
    let fixture = Fixture::new("");
    write_public_dependency_packages(&fixture, helper, demo);
    fixture.commit("seed");
    fixture
}

fn write_public_dependency_packages(fixture: &Fixture, helper: &str, demo: &str) {
    write_package(fixture, "helper", helper, "");
    write_package(
        fixture,
        "demo",
        demo,
        &format!(
            r#"
[package.metadata.cargo_check_external_types]
allowed_external_types = ["helper::*"]

[dependencies]
helper = {{ path = "../helper", version = "{helper}" }}
"#
        ),
    );
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn github_format_emits_workflow_annotations() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 5; }\n");
    fixture.commit("content");
    let base = fixture.sha("HEAD");

    let outcome = run(&RunInput::Check {
        merge_target: None,
        release_history: Some(base),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Github,
        verify_packaging: false,
        config: None,
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Check {
            passed, message, ..
        } => {
            assert!(!passed);
            assert!(message.contains("::error"));
            assert!(message.contains("increment-versions"));
        }
        other => panic!("expected check, got {other:?}"),
    }
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_records_group_verdicts() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "alpha",
        "0.1.0",
        "\n[dependencies]\nbeta = { path = \"../beta\", version = \"=0.1.0\" }\n",
    );
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let report = report_json(&fixture, &base);

    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report.get("schema_version"), Some(&json!(6)));
    assert_eq!(
        report.pointer("/groups/alpha"),
        Some(&json!({
            "members": ["alpha", "beta"],
            "consistent": true,
            "version": "0.1.0"
        }))
    );
    assert_eq!(report.get("non_publishable_packages"), Some(&json!([])));
}

/// A helper-only group is represented entirely by version-target records.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_records_an_all_non_publishable_group() {
    let fixture = Fixture::new("");
    write_package(&fixture, "z-helper", "0.1.0", "\npublish = false\n");
    write_package(
        &fixture,
        "a-helper",
        "0.1.0",
        "\npublish = false\n\n[dependencies]\nz-helper = { path = \"../z-helper\", version = \"=0.1.0\" }\n",
    );
    fixture.commit("helper group");
    let base = fixture.sha("HEAD");

    let report: Value = serde_json::from_str(&report_json(&fixture, &base)).unwrap();

    assert_eq!(report.get("packages"), Some(&json!([])));
    assert_eq!(
        report.get("non_publishable_packages"),
        Some(&json!([
            {
                "name": "a-helper",
                "declared_version": "0.1.0",
                "group": "a-helper"
            },
            {
                "name": "z-helper",
                "declared_version": "0.1.0",
                "group": "a-helper"
            }
        ]))
    );
    assert_eq!(
        report.pointer("/groups/a-helper"),
        Some(&json!({
            "members": ["a-helper", "z-helper"],
            "consistent": true,
            "version": "0.1.0"
        }))
    );
}

/// Explicit wildcard requirements survive packaging and remain report relationships.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_retains_an_explicitly_versioned_dev_dependency() {
    let fixture = Fixture::new("");
    write_package(&fixture, "wildcard_helper", "0.1.0", "");
    write_package(&fixture, "path_only_helper", "0.1.0", "");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        r#"
[dev-dependencies]
wildcard_helper = { path = "../wildcard_helper", version = "*" }
path_only_helper = { path = "../path_only_helper" }
"#,
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let report: Value = serde_json::from_str(&report_json(&fixture, &base)).unwrap();
    let demo = report
        .get("packages")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|package| package.get("name").and_then(Value::as_str) == Some("demo"))
        .unwrap();
    let dependencies = demo.get("dependencies").unwrap();

    assert_eq!(
        dependencies,
        &json!([{
            "name": "wildcard_helper",
            "req": "*",
            "exact_pin": false,
            "public": false
        }])
    );
}

/// Report replaces the diffs of an earlier run.
///
/// A report directory is reused across runs, so a diff left over from a package that no longer has
/// one would still be read as current.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn report_replaces_the_diffs_of_an_earlier_run() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.write("packages/demo/src/lib.rs", "pub fn old() {}\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/src/lib.rs", "pub fn new() {}\n");
    let out_dir = fixture.path().join("out");
    fixture.write("out/report.json", "previous completion marker");
    fixture.write("out/diffs/stale.diff", "leftover");
    let stale = out_dir.join("diffs").join("stale.diff");

    let report: Value = serde_json::from_str(&report_json(&fixture, &base)).unwrap();

    assert!(!stale.exists());
    assert!(out_dir.join("report.json").exists());
    assert!(!out_dir.join("report.json.tmp").exists());
    let diff_path = report
        .pointer("/packages/0/diff_path")
        .unwrap()
        .as_str()
        .unwrap();
    assert_eq!(diff_path, "diffs/demo.patch");
    let patch = fs::read_to_string(out_dir.join(diff_path)).unwrap();
    assert!(patch.contains("-pub fn old() {}"));
    assert!(patch.contains("+pub fn new() {}"));
    assert_eq!(fs::read_dir(out_dir.join("diffs")).unwrap().count(), 1);
}

/// A failed rerun removes the completion marker before changing patches.
///
/// A consumer treats `report.json` as the index of one complete report. Leaving
/// an earlier marker after patch replacement fails would make a mixed artifact
/// set appear complete.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_failed_rerun_does_not_leave_the_previous_report_marker() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    let out_dir = fixture.path().join("out");
    fixture.write("out/report.json", "previous completion marker");
    fixture.write("out/diffs", "blocks directory creation");

    let result = run(&RunInput::Report {
        merge_target: None,
        out_dir: out_dir.clone(),
        release_history: Some(base),
        manifest_path: fixture.manifest(),
        verbose: false,
    });

    result.expect_err("report rerun must fail after deleting tracked content");
    assert!(!out_dir.join("report.json").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "exercises Git/Cargo classification and filesystem staging"
)]
fn staging_failure_does_not_publish_a_completion_marker() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    fixture.write("out/report.json", "previous completion marker");
    let out_dir = fixture.path().join("out");
    fs::create_dir_all(out_dir.join("report.json.tmp")).unwrap();

    let result = run(&RunInput::Report {
        merge_target: None,
        out_dir: out_dir.clone(),
        release_history: Some(base),
        manifest_path: fixture.manifest(),
        verbose: false,
    });

    result.unwrap_err();
    assert!(!out_dir.join("report.json").exists());
    assert!(
        fs::read_to_string(out_dir.join("diffs/demo.patch"))
            .unwrap()
            .contains("+pub fn changed()")
    );
}
