//! Captured-state rejection boundaries and nested workspace input identity.

#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Command;

use cargo_release_plan::{RunInput, run};
#[cfg(windows)]
use serde_json::Value;

use crate::fixture::write_package;
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
