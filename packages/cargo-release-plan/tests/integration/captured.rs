//! Captured-state rejection boundaries and nested workspace input identity.

use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

use cargo_release_plan::{RunInput, run};
#[cfg(windows)]
use serde_json::Value;

use crate::fixture::{Fixture, write_package};
use crate::harness::seeded_package;

#[test]
#[cfg_attr(
    miri,
    ignore = "captures a real Cargo workspace with an absolute local dependency"
)]
fn preparation_rejects_absolute_paths_before_creating_a_resolution_workspace() {
    let fixture = seeded_package();
    write_package(&fixture, "core", "0.1.0", "");
    fixture.commit("local dependency target");
    let manifest = format!(
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\
         [dependencies]\ncore = {{ version = \"0.1.0\", path = {:?} }}\n",
        fixture.path().join("packages/core")
    );
    fixture.write("packages/demo/Cargo.toml", &manifest);
    run(&RunInput::Prepare {
        output: fixture.path().join("prepared"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("prepared/.prospective").exists());
    assert_eq!(fixture.read("packages/demo/Cargo.toml"), manifest);
    assert!(!fixture.path().join("Cargo.lock").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "prepares, previews, and applies through real filesystem case aliases"
)]
fn case_aliased_workspace_inputs_project_and_apply_the_same_final_bytes() {
    let fixture = Fixture::with_workspace_manifest(
        "Rust/Cargo.toml",
        "[workspace]\nmembers = [\"packages/demo\"]\nresolver = \"2\"\n\
         [workspace.package]\nversion = \"0.1.0\"\n",
    );
    fixture.write(
        "Rust/packages/demo/Cargo.toml",
        "[package]\nname = \"demo\"\nversion.workspace = true\nedition = \"2021\"\n",
    );
    fixture.write("Rust/packages/demo/src/lib.rs", "pub fn released() {}\n");
    if !fixture
        .path()
        .join("rust/packages/DEMO/CARGO.TOML")
        .exists()
    {
        eprintln!("This filesystem does not provide the case aliases required by this scenario.");
        return;
    }
    for (from, to) in [
        ("Rust/Cargo.toml", "Rust/cargo.toml"),
        (
            "Rust/packages/demo/Cargo.toml",
            "Rust/packages/demo/cargo.toml",
        ),
        ("Rust", "rust"),
    ] {
        fixture.rename_case(from, to);
    }
    fixture.commit("released workspace");
    fixture.rename_case("rust", "Rust");
    fixture.rename_case("Rust/packages/demo", "Rust/packages/Demo");
    let index = fixture.git(&["ls-files", "--stage", "-z"]);
    let prepared = fixture.path().join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fixture.write(
        "proposal.json",
        r#"{"schema_version":4,"increments":[{"name":"demo","level":"patch"}]}"#,
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
    let apply = || {
        run(&RunInput::Apply {
            plan: preview.join("plan.json"),
            dry_run: false,
            manifest_path: fixture.manifest(),
            verbose: false,
        })
        .unwrap();
    };
    apply();
    assert!(
        fixture
            .read("Rust/packages/Demo/Cargo.toml")
            .contains("version = \"0.1.1\"")
    );
    let final_lock = fs::read(fixture.path().join("Rust/Cargo.lock")).unwrap();
    apply();
    assert_eq!(
        fs::read(fixture.path().join("Rust/Cargo.lock")).unwrap(),
        final_lock
    );
    assert_eq!(fixture.git(&["ls-files", "--stage", "-z"]), index);
}

#[test]
#[cfg(windows)]
#[cfg_attr(
    miri,
    ignore = "uses Windows short paths and native PowerShell/Git/Cargo"
)]
fn short_windows_paths_use_the_same_captured_workspace_identity() {
    let fixture = seeded_package();
    // PowerShell is part of the repository's Windows test environment. The filesystem API
    // probes actual short-name availability instead of assuming the volume provides it.
    fixture.write(
        "short-path.ps1",
        "# Returns the actual short-name spelling for this test's owned directory.\n\
         param([string] $Path)\n\
         Set-StrictMode -Version Latest\n\
         $ErrorActionPreference = 'Stop'\n\
         $PSNativeCommandUseErrorActionPreference = $true\n\
         $filesystem = New-Object -ComObject Scripting.FileSystemObject\n\
         $filesystem.GetFolder($Path).ShortPath\n",
    );
    let output = Command::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(fixture.path().join("short-path.ps1"))
        .arg(fixture.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let short_root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
    assert_eq!(
        fs::canonicalize(&short_root).unwrap(),
        fs::canonicalize(fixture.path()).unwrap()
    );
    if short_root == fixture.path() {
        eprintln!("This volume exposes no distinct short directory name.");
        return;
    }
    let manifest = short_root.join("Cargo.toml");
    let prepared = short_root.join("prepared");
    run(&RunInput::Prepare {
        output: prepared.clone(),
        base: Some("HEAD".to_owned()),
        manifest_path: manifest,
        verbose: false,
    })
    .unwrap();
    let document: Value =
        serde_json::from_slice(&fs::read(prepared.join("prepared.json")).unwrap()).unwrap();
    let captured_root = Path::new(document.pointer("/inputs/root").unwrap().as_str().unwrap());
    assert_eq!(
        fs::canonicalize(captured_root).unwrap(),
        fs::canonicalize(fixture.path()).unwrap()
    );
    assert_eq!(
        Path::new(
            document
                .pointer("/inputs/manifest")
                .unwrap()
                .as_str()
                .unwrap()
        ),
        Path::new("Cargo.toml")
    );
}
