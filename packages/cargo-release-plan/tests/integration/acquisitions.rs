//! Operation-local reuse, independent acquisitions and controlled mutation boundaries.

#[cfg(unix)]
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::resolved::Inputs;
use serde_json::json;
use tempfile::TempDir;

use crate::fixture::{Fixture, write_package};
use crate::harness::seeded_package;

fn command(fixture: &Fixture) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
    command
        .current_dir(fixture.path())
        .env_remove("CARGO_TARGET_DIR");
    command
}

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn report(fixture: &Fixture, output: &Path, trace: &Path, options: &[&str]) -> Output {
    success(
        command(fixture)
            .args(["report", "--release-history", "HEAD", "--out-dir"])
            .arg(output)
            .args(options)
            .env("GIT_TRACE", trace)
            .output()
            .unwrap(),
    )
}

fn acquisitions(trace: &Path) -> (usize, usize) {
    let trace = fs::read_to_string(trace).unwrap();
    (
        trace
            .lines()
            .filter(|line| line.contains("built-in: git ls-tree -r -z "))
            .count(),
        trace
            .lines()
            .filter(|line| line.contains("built-in: git cat-file -p "))
            .count(),
    )
}

fn assert_reports_equal(expected: &Path, actual: &Path) {
    assert_eq!(
        fs::read(expected.join("report.json")).unwrap(),
        fs::read(actual.join("report.json")).unwrap()
    );
    for entry in fs::read_dir(expected.join("diffs")).unwrap() {
        let entry = entry.unwrap();
        assert_eq!(
            fs::read(entry.path()).unwrap(),
            fs::read(actual.join("diffs").join(entry.file_name())).unwrap()
        );
    }
}

#[test]
#[cfg_attr(miri, ignore = "executes Git, Cargo and the compiled application")]
fn independent_reports_reacquire_inputs_and_share_each_immutable_snapshot() {
    let fixture = Fixture::new("");
    for name in ["first", "second", "third"] {
        write_package(&fixture, name, "0.1.0", "");
    }
    fixture.commit("shared root anchor");
    fixture.write("packages/first/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let original = Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap();
    // Existing disposable data is user-owned; commands neither consult nor remove it.
    let storage = fixture.path().join("target/cargo-release-plan/cache");
    fs::create_dir_all(&storage).unwrap();
    fs::write(storage.join("untouched"), "old data").unwrap();
    for name in ["first", "second"] {
        let trace = evidence.path().join(format!("{name}.trace"));
        report(&fixture, &evidence.path().join(name), &trace, &[]);
        assert_eq!(acquisitions(&trace), (1, 1));
        let trace = fs::read_to_string(&trace).unwrap();
        for operation in [
            "git ls-files -z -- ",
            "git ls-files -s -z -- ",
            "git diff-files --raw -z --no-renames -- ",
            "git ls-files -z --others --exclude-standard -- ",
            "git cat-file --batch-check",
        ] {
            assert_eq!(
                trace
                    .lines()
                    .filter(|line| line.contains(operation))
                    .count(),
                1,
                "{trace}"
            );
        }
        // Historical manifests use one batch, and the changed patch uses another.
        // Independent commands reacquire both; repeated endpoints share each bounded batch.
        assert_eq!(
            trace
                .lines()
                .filter(|line| line.ends_with("git cat-file --batch"))
                .count(),
            2,
            "{trace}"
        );
    }
    assert_reports_equal(
        &evidence.path().join("first"),
        &evidence.path().join("second"),
    );
    original.verify(&fixture.manifest(), None).unwrap();
    assert_eq!(
        fs::read_to_string(storage.join("untouched")).unwrap(),
        "old data"
    );
    fixture.write("packages/second/src/lib.rs", "pub fn another_change() {}\n");
    report(
        &fixture,
        &evidence.path().join("changed"),
        &evidence.path().join("changed.trace"),
        &[],
    );
    assert_ne!(
        fs::read(evidence.path().join("first/report.json")).unwrap(),
        fs::read(evidence.path().join("changed/report.json")).unwrap()
    );
    let error = original.verify(&fixture.manifest(), None).unwrap_err();
    assert!(error.to_string().contains("stale"));
}

#[test]
#[cfg(unix)]
#[cfg_attr(
    miri,
    ignore = "executes Git with an unrepresentable replacement namespace"
)]
fn non_utf8_replacement_namespace_is_not_an_unset_variable() {
    let fixture = seeded_package();
    let evidence = TempDir::new().unwrap();
    report(
        &fixture,
        &evidence.path().join("baseline"),
        &evidence.path().join("baseline.trace"),
        &[],
    );
    let namespace = OsString::from_vec(b"refs/replacements-\xff/".to_vec());
    let output = command(&fixture)
        .args(["check", "--release-history", "HEAD"])
        .env("GIT_REPLACE_REF_BASE", &namespace)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("GIT_REPLACE_REF_BASE"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes preparation, prospective resolution and source verification"
)]
fn workflow_reuses_admission_and_reacquires_after_resolution_and_relocation() {
    let fixture = seeded_package();
    fixture.write(
        "packages/demo/Cargo.toml",
        &format!(
            "{}\n[package.metadata.release-plan]\nprivate-api = true\n",
            fixture.read("packages/demo/Cargo.toml")
        ),
    );
    fixture.commit("private API configuration");
    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let prepared = evidence.path().join("prepared");
    let prepare_trace = evidence.path().join("prepare.trace");
    success(
        command(&fixture)
            .args(["prepare", "--release-history", "HEAD", "--output"])
            .arg(&prepared)
            .env("GIT_TRACE", &prepare_trace)
            .output()
            .unwrap(),
    );
    // Original entry, resolved temporary workspace, and
    // original post-install capture.
    // The post-install classification consumes that capture rather than acquiring again.
    let prepare_trace = fs::read_to_string(prepare_trace).unwrap();
    assert_eq!(
        prepare_trace
            .lines()
            .filter(|line| line.ends_with("git ls-files -z -- ':(literal).'"))
            .count(),
        3,
    );
    assert_eq!(
        prepare_trace
            .lines()
            .filter(|line| line.contains("git ls-files --stage -z"))
            .count(),
        2
    );
    assert_eq!(
        prepare_trace
            .lines()
            .filter(|line| line.contains("git ls-files -s -z -- "))
            .count(),
        0
    );
    assert_eq!(
        prepare_trace
            .lines()
            .filter(|line| line.contains("git diff-files --raw"))
            .count(),
        1
    );
    let plan = evidence.path().join("proposal.json");
    fs::write(
        &plan,
        serde_json::to_vec(&json!({
            "schema_version": SCHEMA_VERSION,
            "increments": [{"name": "demo", "bump": "patch"}]
        }))
        .unwrap(),
    )
    .unwrap();
    let preview = evidence.path().join("preview");
    let trace = evidence.path().join("preview.trace");
    success(
        command(&fixture)
            .args(["preview", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--plan")
            .arg(&plan)
            .arg("--output")
            .arg(&preview)
            .env("GIT_TRACE", &trace)
            .output()
            .unwrap(),
    );
    // The history tip and package version anchor are different commits. Each tree is
    // acquired once, then shared across the candidate's classification passes.
    assert_eq!(acquisitions(&trace), (2, 1));
    // Original admission, candidate creation, each convergence
    // pass, and final relocation.
    assert_eq!(
        fs::read_to_string(&trace)
            .unwrap()
            .lines()
            .filter(|line| line.ends_with("git ls-files -z -- ':(literal).'"))
            .count(),
        5,
    );
    let resolved: serde_json::Value =
        serde_json::from_slice(&fs::read(preview.join("plan.json")).unwrap()).unwrap();
    let manifest = PathBuf::from(
        resolved
            .get("resolved")
            .unwrap()
            .get("evidence_manifest_path")
            .unwrap()
            .as_str()
            .unwrap(),
    );
    let candidate_manifest =
        fs::read_to_string(manifest.parent().unwrap().join("packages/demo/Cargo.toml")).unwrap();
    assert!(candidate_manifest.contains("0.1.1"), "{candidate_manifest}");
    assert_eq!(
        fs::read_to_string(manifest.parent().unwrap().join("packages/demo/src/lib.rs")).unwrap(),
        "pub fn changed() {}\n"
    );
    let compatibility_trace = evidence.path().join("compatibility.trace");
    success(
        command(&fixture)
            .args(["check-compatibility", "--plan"])
            .arg(preview.join("plan.json"))
            .arg("--output")
            .arg(evidence.path().join("compatibility"))
            .env("GIT_TRACE", &compatibility_trace)
            .output()
            .unwrap(),
    );
    assert_eq!(acquisitions(&compatibility_trace), (2, 1));
    success(
        command(&fixture)
            .args(["verify-preview", "--plan"])
            .arg(preview.join("plan.json"))
            .arg("--manifest-path")
            .arg(&manifest)
            .output()
            .unwrap(),
    );
    success(
        command(&fixture)
            .args(["apply", "--dry-run", "--plan"])
            .arg(preview.join("plan.json"))
            .env("GIT_TRACE", evidence.path().join("apply.trace"))
            .output()
            .unwrap(),
    );
    // Dry-run application has one explicit source verification. Its metadata and retained
    // listing also serve plan/artifact validation, rather than starting another capture.
    let trace = fs::read_to_string(evidence.path().join("apply.trace")).unwrap();
    assert_eq!(
        trace
            .lines()
            .filter(|line| line.contains("built-in: git ls-files -z -- "))
            .count(),
        1
    );

    // An independent command must admit the current source before consuming retained evidence.
    fixture.write_workspace("[workspace.dependencies]\nunused = { path = 'unavailable' }\n");
    let output = command(&fixture)
        .args(["check-compatibility", "--plan"])
        .arg(preview.join("plan.json"))
        .arg("--output")
        .arg(evidence.path().join("stale"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("stale"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes independent checks and real Cargo packaging probes"
)]
fn packaging_probes_reuse_their_classification_listing_but_commands_reacquire() {
    let fixture = Fixture::new("[workspace.package]\nreadme='README.md'\n");
    for name in ["first", "second"] {
        write_package(&fixture, name, "0.1.0", "readme.workspace=true\n");
    }
    fixture.write("README.md", "shared\n");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("packaging baseline");
    let evidence = TempDir::new().unwrap();
    for pass in 0..2 {
        let trace = evidence.path().join(format!("check-{pass}.trace"));
        let output = success(
            command(&fixture)
                .args(["check", "--release-history", "HEAD", "--verify-packaging"])
                .env("GIT_TRACE", &trace)
                .output()
                .unwrap(),
        );
        assert!(
            output.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let trace = fs::read_to_string(trace).unwrap();
        for operation in [
            "git ls-files -z -- ",
            "git ls-files -s -z -- ",
            "git diff-files --raw -z --no-renames -- ",
        ] {
            assert_eq!(
                trace
                    .lines()
                    .filter(|line| line.contains(operation))
                    .count(),
                1,
                "{trace}"
            );
        }
        // Each independent command must see the staged addition; Cargo's real package list
        // includes it, so retaining the preceding command's listing would produce a mismatch.
        fixture.write("packages/first/src/new.rs", "pub fn added() {}\n");
        fixture.write(
            "packages/first/Cargo.toml",
            &fixture
                .read("packages/first/Cargo.toml")
                .replace("0.1.0", "0.1.1"),
        );
        fixture.git(&["add", "packages/first"]);
        fixture.cargo(&["generate-lockfile", "--offline"]);
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes a stateful Git clean filter and the application"
)]
fn shared_resource_filters_keep_each_packages_original_conversion_and_process_boundary() {
    let fixture = Fixture::new("[workspace.package]\nreadme='README.md'\n");
    for name in ["first", "second"] {
        write_package(&fixture, name, "0.1.0", "readme.workspace=true\n");
    }
    fixture.write("README.md", "original\n");
    fixture.commit("shared resource");
    fixture.git(&["update-index", "--refresh"]);
    // A synthetic old index timestamp forces Git's racy-clean content check without a clock
    // or sleep. Even raw mode queries then execute the filter, before each package's hash.
    fs::OpenOptions::new()
        .write(true)
        .open(fixture.path().join(".git/index"))
        .unwrap()
        .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(1))
        .unwrap();
    fixture.write(".git/info/attributes", "README.md filter=count\n");
    // Git owns invoking this fixture; rendering must not repeat its stateful conversion.
    fixture.write(
        ".git/clean.ps1",
        r#"#requires -Version 7.6
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
# Git clean fixture: count each conversion of a shared released resource.
$null = [Console]::In.ReadToEnd()
$path = '.git/filter-count'
$count = if (Test-Path -LiteralPath $path) { [int][IO.File]::ReadAllText($path) } else { 0 }
$count += 1
[IO.File]::WriteAllText($path, [string]$count)
[Console]::Write("converted-$count`n")
"#,
    );
    fixture.git(&[
        "config",
        "filter.count.clean",
        "pwsh -NoProfile -File .git/clean.ps1",
    ]);
    fixture.git(&["config", "filter.count.required", "true"]);
    let evidence = TempDir::new().unwrap();
    for pass in 0..2 {
        let output = evidence.path().join(format!("pass-{pass}"));
        let trace = evidence.path().join(format!("pass-{pass}.trace"));
        report(&fixture, &output, &trace, &[]);
        let trace = fs::read_to_string(trace).unwrap();
        let hashes: Vec<_> = trace
            .lines()
            .filter(|line| line.contains("git hash-object -w --"))
            .collect();
        assert_eq!(hashes.len(), 2, "{trace}");
        assert_eq!(
            trace
                .lines()
                .filter(|line| line.contains("git diff-files --raw"))
                .count(),
            2,
            "{trace}"
        );
        // Discovery need not enumerate package directories in alphabetical order.
        // Each patch uses the conversion from that package's own hash process.
        for (index, command) in hashes.iter().enumerate() {
            let name = ["first", "second"]
                .into_iter()
                .find(|name| command.contains(&format!("{name}/Cargo.toml")))
                .unwrap();
            let patch = fs::read_to_string(output.join(format!("diffs/{name}.patch"))).unwrap();
            assert!(
                patch.contains(&format!("+converted-{}\n", pass * 4 + index * 2 + 2)),
                "pass {pass}, package {name}, count {}\n{patch}\n{trace}",
                fixture.read(".git/filter-count")
            );
        }
        assert_eq!(
            fixture.read(".git/filter-count"),
            (pass * 4 + 4).to_string()
        );
    }
}
