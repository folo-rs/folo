//! Captured build inputs selected outside conventional member source directories.

use std::fs;
use std::path::Path;
use std::process::Command;

use cargo_release_plan::{RunInput, run};
use crp_versioning::preview::Prepared;
use serde_json::Value;
use tempfile::tempdir;

use crate::fixture::{Fixture, write_package};
use crate::harness::seeded_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a discovered dependency's explicitly selected build script"
)]
fn dependency_explicit_build_file_is_captured() {
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
    let output = fixture.path().join("prepared-evidence");
    run(&RunInput::Prepare {
        output: output.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let prepared: Prepared =
        serde_json::from_slice(&fs::read(output.join("prepared.json")).unwrap()).unwrap();
    assert!(
        prepared
            .inputs
            .paths
            .contains(Path::new("evidence/diffs/build.rs"))
    );
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

#[test]
#[cfg_attr(
    miri,
    ignore = "builds original and retained custom-target dependencies"
)]
fn dedicated_target_directories_reconstruct_and_verify_ignored_modules() {
    let fixture = dependency_targets("custom/lib.rs", "build-sources/nested/../build.rs");
    fixture.cargo(&["check", "--offline", "--quiet"]);
    fixture.commit("select dedicated targets");

    let prepared = check_retained_targets(&fixture);

    for path in [
        "helper/custom/library_helper.rs",
        "helper/custom/library_helper/nested.rs",
        "helper/build-sources/build_helper.rs",
    ] {
        assert!(prepared.inputs.paths.contains(Path::new(path)));
        assert!(fixture.git(&["ls-files", "--", path]).is_empty());
        let before = fixture.read(path);
        fixture.write(path, &format!("{before}\n// changed captured module\n"));
        prepared
            .inputs
            .verify(&fixture.manifest(), None)
            .unwrap_err();
        fixture.write(path, &before);
        prepared.inputs.verify(&fixture.manifest(), None).unwrap();
    }
    for path in ["helper/unselected.txt", "support/unselected.txt"] {
        assert!(!prepared.inputs.paths.contains(Path::new(path)));
    }
}

#[test]
#[cfg_attr(miri, ignore = "reconstructs Git-added support files without a commit")]
fn targets_outside_dedicated_directories_use_git_added_support_files() {
    for directory in ["", "../support/"] {
        let fixture = dependency_targets(
            &format!("{directory}lib.rs"),
            &format!("{directory}build.rs"),
        );
        fixture.cargo(&["check", "--offline", "--quiet"]);
        fixture.commit("select root and outside targets");
        let source = if directory.is_empty() {
            "helper"
        } else {
            "support"
        };
        let modules = [
            format!("{source}/library_helper.rs"),
            format!("{source}/library_helper/nested.rs"),
            format!("{source}/build_helper.rs"),
        ];
        for path in &modules {
            assert!(
                fixture
                    .git(&["ls-tree", "-r", "--name-only", "HEAD", "--", path])
                    .is_empty()
            );
            fixture.git(&["add", "--force", "--", path]);
        }
        let prepared = check_retained_targets(&fixture);
        for path in &modules {
            assert!(prepared.inputs.paths.contains(Path::new(path)));
            let before = fixture.read(path);
            fixture.write(path, &format!("{before}\n// changed tracked support\n"));
            prepared
                .inputs
                .verify(&fixture.manifest(), None)
                .unwrap_err();
            fixture.write(path, &before);
            prepared.inputs.verify(&fixture.manifest(), None).unwrap();
        }
        for path in ["helper/unselected.txt", "support/unselected.txt"] {
            assert!(!prepared.inputs.paths.contains(Path::new(path)));
        }
    }
}

// Ignored support modules and unrelated files distinguish dedicated-directory capture
// from support explicitly added to the current index.
fn dependency_targets(library: &str, build: &str) -> Fixture {
    let fixture = seeded_package();
    fixture.write_workspace("exclude=['helper']");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "[dependencies]\nhelper={path='../../helper'}",
    );
    fixture.write(
        "packages/demo/src/lib.rs",
        "pub fn value() -> u8 { helper::value() }\n",
    );
    fixture.write(
        ".gitignore",
        "**/target/\n**/library_helper.rs\n**/library_helper/\n**/build_helper.rs\n**/unselected.txt\n",
    );
    fixture.write(
        "helper/Cargo.toml",
        &format!(
            "[package]\nname='helper'\nversion='0.1.0'\nedition='2024'\nbuild='{build}'\n\
             [lib]\npath='{library}'\n[workspace]\n"
        ),
    );
    fixture.write(
        &format!("helper/{library}"),
        "mod library_helper; pub fn value() -> u8 { library_helper::value() }\n",
    );
    fixture.write(
        &format!("helper/{build}"),
        "mod build_helper; fn main() { build_helper::build(); }\n",
    );
    let library_directory = Path::new(library).parent().unwrap();
    let build_directory = Path::new(build).parent().unwrap();
    for (directory, name, contents) in [
        (
            library_directory,
            "library_helper.rs",
            "mod nested; pub fn value() -> u8 { nested::value() }\n",
        ),
        (
            library_directory,
            "library_helper/nested.rs",
            "pub fn value() -> u8 { 1 }\n",
        ),
        (build_directory, "build_helper.rs", "pub fn build() {}\n"),
    ] {
        fixture.write(
            Path::new("helper")
                .join(directory)
                .join(name)
                .to_str()
                .unwrap(),
            contents,
        );
    }
    fixture.write("helper/unselected.txt", "not a declared or tracked source");
    fixture.write("support/unselected.txt", "not a declared or tracked source");
    fixture
}

// Build the retained workspace to verify reconstruction, then return original preparation
// so callers can verify that later source changes invalidate its captured inputs.
fn check_retained_targets(fixture: &Fixture) -> Prepared {
    let output = tempdir().unwrap();
    let prepared = output.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        release_history: Some("HEAD".to_owned()),
        merge_target: None,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let plan = output.path().join("proposal.json");
    fs::write(
        &plan,
        r#"{"schema_version":6,"increments":[{"name":"demo","bump":"minor"}]}"#,
    )
    .unwrap();
    let preview = output.path().join("preview");
    run(&RunInput::Preview {
        plan,
        prepared: prepared.join("prepared.json"),
        output: preview.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let result = Command::new("cargo")
        .args([
            "check",
            "--offline",
            "--locked",
            "--quiet",
            "--manifest-path",
        ])
        .arg(preview.join("workspace/Cargo.toml"))
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    serde_json::from_slice(&fs::read(prepared.join("prepared.json")).unwrap()).unwrap()
}
