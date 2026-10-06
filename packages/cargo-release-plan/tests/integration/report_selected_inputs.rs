//! Native selection and publication regressions for nonconventional source locations.

use std::fs;

use cargo_release_plan::{RunInput, run};
use serde_json::Value;
use tempfile::tempdir;

use crate::fixture::write_package;
use crate::harness::seeded_package;
use crate::report::{directory_alias, report_command};

#[test]
#[cfg_attr(
    miri,
    ignore = "protects a Cargo-selected dependency alias during report cleanup"
)]
fn report_preserves_declared_dependency_alias_entries() {
    let fixture = seeded_package();
    let helper = tempdir().unwrap();
    fs::create_dir_all(helper.path().join("src")).unwrap();
    fs::write(
        helper.path().join("Cargo.toml"),
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    )
    .unwrap();
    fs::write(helper.path().join("src/lib.rs"), "pub fn helper() {}\n").unwrap();
    fixture.write_workspace("exclude=['evidence/diffs/helper']");
    fixture.write(".gitignore", "evidence/\n");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../evidence/diffs/helper'}",
    );
    fixture.write("evidence/report.json", "previous completion");
    let alias = fixture.path().join("evidence/diffs/helper");
    fs::create_dir_all(alias.parent().unwrap()).unwrap();
    directory_alias(helper.path(), &alias);
    let metadata: Value =
        serde_json::from_str(&fixture.cargo(&["metadata", "--offline", "--format-version", "1"]))
            .unwrap();
    assert!(
        metadata
            .get("packages")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|package| { package.get("name").unwrap() == "helper" })
    );
    fixture.commit("select ignored dependency alias");
    let output = fixture.path().join("evidence");
    let result = report_command(&fixture, &output).output().unwrap();
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(fixture.read("evidence/report.json"), "previous completion");
    assert_eq!(
        fs::canonicalize(&alias).unwrap(),
        fs::canonicalize(helper.path()).unwrap()
    );
    let accepted = report_command(&fixture, &helper.path().join("evidence"))
        .output()
        .unwrap();
    assert!(accepted.status.success(), "{accepted:?}");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks Cargo-selected explicit target files before native report reset"
)]
fn report_preserves_explicit_build_and_target_files() {
    for declaration in [
        "build='evidence/diffs/selected.rs'\n",
        "[lib]\npath='evidence/diffs/selected.rs'\n",
        "[[bin]]\nname='selected'\npath='evidence/diffs/selected.rs'\n",
        "[[example]]\nname='selected'\npath='evidence/diffs/selected.rs'\n",
        "[[test]]\nname='selected'\npath='evidence/diffs/selected.rs'\n",
        "[[bench]]\nname='selected'\npath='evidence/diffs/selected.rs'\n",
    ] {
        let fixture = seeded_package();
        write_package(&fixture, "demo", "0.1.0", declaration);
        fixture.write(".gitignore", "packages/demo/evidence/\n");
        fixture.write("packages/demo/evidence/diffs/selected.rs", "fn main() {}\n");
        fixture.write("packages/demo/evidence/report.json", "previous completion");
        let source = fixture
            .path()
            .join("packages/demo/evidence/diffs/selected.rs");
        let metadata: Value = serde_json::from_str(&fixture.cargo(&[
            "metadata",
            "--no-deps",
            "--offline",
            "--format-version",
            "1",
        ]))
        .unwrap();
        assert!(
            metadata
                .pointer("/packages/0/targets")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .any(|target| {
                    fs::canonicalize(target.get("src_path").unwrap().as_str().unwrap()).unwrap()
                        == fs::canonicalize(&source).unwrap()
                })
        );
        fixture.cargo(&["generate-lockfile", "--offline"]);
        fixture.commit("select ignored target");
        let output = fixture.path().join("packages/demo/evidence");
        let result = report_command(&fixture, &output).output().unwrap();
        assert!(!result.status.success(), "{declaration}: {result:?}");
        assert_eq!(fs::read_to_string(source).unwrap(), "fn main() {}\n");
        assert_eq!(
            fixture.read("packages/demo/evidence/report.json"),
            "previous completion"
        );
        let accepted = report_command(&fixture, &output.join("disjoint"))
            .output()
            .unwrap();
        assert!(accepted.status.success(), "{declaration}: {accepted:?}");
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "protects a discovered dependency's explicitly selected build script"
)]
fn report_preserves_dependency_build_files() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude=['helper']");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../helper'}",
    );
    fixture.write(".gitignore", "evidence/\n");
    fixture.write(
        "helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\nbuild='../evidence/diffs/build.rs'\n[workspace]\n",
    );
    fixture.write("helper/src/lib.rs", "pub fn helper() {}\n");
    fixture.write("evidence/diffs/build.rs", "fn main() {}\n");
    fixture.write("evidence/report.json", "previous completion");
    let metadata: Value =
        serde_json::from_str(&fixture.cargo(&["metadata", "--offline", "--format-version", "1"]))
            .unwrap();
    let helper = metadata
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package.get("name").unwrap() == "helper")
        .unwrap();
    assert!(
        helper
            .get("targets")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .any(|target| {
                target
                    .get("kind")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|kind| kind == "custom-build")
            })
    );
    fixture.commit("select dependency build");
    let result = report_command(&fixture, &fixture.path().join("evidence"))
        .output()
        .unwrap();
    assert!(!result.status.success(), "{result:?}");
    assert_eq!(fixture.read("evidence/diffs/build.rs"), "fn main() {}\n");
    assert_eq!(fixture.read("evidence/report.json"), "previous completion");

    let disjoint = fixture.path().join("prepared-evidence");
    run(&RunInput::Prepare {
        output: disjoint.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert!(disjoint.join("prepared.json").exists());
}

#[test]
#[cfg_attr(miri, ignore = "checks report staging with a real hard-linked source")]
fn report_staging_never_truncates_preexisting_shared_file_contents() {
    let fixture = seeded_package();
    let output = tempdir().unwrap();
    let source = fixture.path().join("packages/demo/src/lib.rs");
    let before = fs::read(&source).unwrap();
    fs::hard_link(&source, output.path().join("report.json.tmp")).unwrap();
    let result = report_command(&fixture, output.path()).output().unwrap();
    assert!(result.status.success(), "{result:?}");
    assert_eq!(fs::read(source).unwrap(), before);
    assert!(!output.path().join("report.json.tmp").exists());
    let report: Value =
        serde_json::from_slice(&fs::read(output.path().join("report.json")).unwrap()).unwrap();
    assert_eq!(report.pointer("/packages/0/status").unwrap(), "unchanged");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "retains and verifies Cargo's implicit nonmember build-script input"
)]
fn dependency_default_build_script_is_captured_and_verified() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude=['helper']");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../helper'}",
    );
    fixture.write(".gitignore", "helper/build.rs\n");
    fixture.write(
        "helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\n[workspace]\n",
    );
    fixture.write("helper/src/lib.rs", "pub fn helper() {}\n");
    fixture.write("helper/build.rs", "fn main() {}\n");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("select conventional dependency build");
    let prepared = fixture.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.write(
        "proposal.json",
        r#"{"schema_version":6,"increments":[{"name":"demo","bump":"minor"}]}"#,
    );
    let preview = fixture.path().join("preview");
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared: prepared.join("prepared.json"),
        output: preview.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert_eq!(
        fs::read_to_string(preview.join("workspace/helper/build.rs")).unwrap(),
        "fn main() {}\n",
    );
    fixture.write("helper/build.rs", "fn main() { println!(\"changed\"); }\n");
    let rejected = fixture.path().join("rejected");
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared: prepared.join("prepared.json"),
        output: rejected.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!rejected.exists());
}
