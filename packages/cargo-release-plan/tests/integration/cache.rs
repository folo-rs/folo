//! Executable cache selection, persistence and evidence independence over hermetic repositories.

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};
use std::{fs, io, thread};

use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::resolved::Inputs;
use crp_workspace::manifest::PathCase;
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

pub(crate) fn entries(directory: &Path, subject: &str) -> Vec<PathBuf> {
    fs::read_dir(directory.join(subject))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
}

fn assert_reuse_diagnostics(name: &str, output: &Output) {
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        diagnostics.contains("acquiring manifest-document "),
        name == "cold"
    );
    assert_eq!(
        diagnostics.contains("computed classification decisions"),
        name == "cold"
    );
    assert_eq!(
        diagnostics.contains("reusing classification decisions from storage"),
        name == "warm"
    );
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
fn persistent_observations_eliminate_duplicate_git_acquisitions_without_changing_reports() {
    let fixture = Fixture::new("");
    for name in ["first", "second", "third"] {
        write_package(&fixture, name, "0.1.0", "");
    }
    fixture.commit("shared root anchor");
    fixture.write("packages/first/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let original = Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap();
    let cold = evidence.path().join("cold");
    let warm = evidence.path().join("warm");
    let disabled = evidence.path().join("disabled");
    for (name, output, options) in [
        ("cold", &cold, &[][..]),
        ("warm", &warm, &[][..]),
        ("disabled", &disabled, &["--no-cache"][..]),
    ] {
        let trace = evidence.path().join(format!("{name}.trace"));
        let mut options = options.to_vec();
        options.push("--verbose");
        let result = report(&fixture, output, &trace, &options);
        assert_reuse_diagnostics(name, &result);
        let trace_text = fs::read_to_string(&trace).unwrap();
        for operation in [
            "git ls-files -s -z -- ",
            "git diff-files --raw -z --no-renames -- ",
            "git ls-files -z --others --exclude-standard -- ",
        ] {
            assert_eq!(
                trace_text
                    .lines()
                    .filter(|line| line.contains(operation))
                    .count(),
                1
            );
        }
        // Disabled storage has only the metadata pass's tracked listing. Enabled storage
        // also admits its directory against tracked source before that classification pass.
        assert_eq!(
            trace_text
                .lines()
                .filter(|line| line.contains("git ls-files -z -- "))
                .count(),
            if name == "disabled" { 1 } else { 2 }
        );
        // Historical manifests use one separate batch. A warm cache removes the patch's
        // content batch, while size queries remain fresh availability observations.
        assert_eq!(
            trace_text
                .lines()
                .filter(|line| line.ends_with("git cat-file --batch"))
                .count(),
            if name == "warm" { 1 } else { 2 }
        );
        assert_eq!(
            acquisitions(&trace),
            if name == "warm" { (0, 0) } else { (1, 1) }
        );
    }
    for output in [&warm, &disabled] {
        assert_reports_equal(&cold, output);
    }
    original.verify(&fixture.manifest(), None).unwrap();
    let storage = fixture
        .path()
        .join("target")
        .join("cargo-release-plan")
        .join("cache");
    assert_eq!(entries(&storage, "git-trees").len(), 1);
    assert_eq!(entries(&storage, "git-parent-headers").len(), 1);
    assert_eq!(entries(&storage, "git-blob-batches").len(), 1);
    assert!(!entries(&storage, "manifest-document").is_empty());
    assert_eq!(entries(&storage, "classification-decisions").len(), 1);
    assert!(
        !fixture
            .git(&["status", "--porcelain", "--untracked-files=all"])
            .contains("cache")
    );
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
    // This executable fixture makes repeated conversion observable without a clock.
    // Git owns invoking it; the test verifies that rendering never runs the driver again.
    fixture.write(
        ".git/clean.ps1",
        r#"#requires -Version 7.6
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"
$PSNativeCommandUseErrorActionPreference = $true
# Git clean fixture for cache tests: count each conversion of a shared released resource.
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
        let result = report(&fixture, &output, &trace, &["--verbose"]);
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("computed classification decisions")
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("reusing classification decisions")
        );
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
        // Each patch must use the conversion from that package's own hash process.
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

#[test]
#[cfg_attr(
    miri,
    ignore = "executes Cargo configuration and process environment resolution"
)]
fn cargo_target_configuration_environment_and_explicit_invocation_relative_override() {
    let fixture = seeded_package();
    fixture.write(
        ".cargo/config.toml",
        "[build]\ntarget-dir = 'configured-target'\n",
    );
    fixture.commit("target configuration");
    let evidence = TempDir::new().unwrap();
    report(
        &fixture,
        &evidence.path().join("configured"),
        &evidence.path().join("configured.trace"),
        &[],
    );
    let configured = fixture
        .path()
        .join("configured-target")
        .join("cargo-release-plan")
        .join("cache");
    assert!(!entries(&configured, "git-trees").is_empty());

    let environmental = evidence.path().join("environment-target");
    success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD"])
            .env("CARGO_TARGET_DIR", &environmental)
            .output()
            .unwrap(),
    );
    assert!(
        !entries(
            &environmental.join("cargo-release-plan").join("cache"),
            "git-trees"
        )
        .is_empty()
    );

    let invocation = evidence.path().join("invocation");
    fs::create_dir_all(&invocation).unwrap();
    success(
        command(&fixture)
            .current_dir(&invocation)
            .args(["check", "--release-history", "HEAD", "--manifest-path"])
            .arg(fixture.manifest())
            .args(["--cache", "relative-cache"])
            .output()
            .unwrap(),
    );
    assert!(!entries(&invocation.join("relative-cache"), "git-trees").is_empty());
    assert!(!fixture.path().join("relative-cache").exists());

    let absolute = evidence.path().join("absolute-cache");
    success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD", "--cache"])
            .arg(&absolute)
            .output()
            .unwrap(),
    );
    assert!(!entries(&absolute, "git-trees").is_empty());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes the application and examines disposable files"
)]
fn disabled_cache_does_not_create_storage_and_conflicts_do_not_write_source() {
    let fixture = seeded_package();
    success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD", "--no-cache"])
            .output()
            .unwrap(),
    );
    assert!(
        !fixture
            .path()
            .join("target")
            .join("cargo-release-plan")
            .exists()
    );
    let original = fixture.read("packages/demo/src/lib.rs");
    for path in [
        "packages/demo/src/cache",
        "packages/demo",
        "Cargo.toml",
        ".git",
        ".git/unused-cache",
    ] {
        let output = command(&fixture)
            .args(["check", "--release-history", "HEAD", "--cache", path])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fixture.read("packages/demo/src/lib.rs"), original);
    }
    let output = command(&fixture)
        .args([
            "check",
            "--release-history",
            "HEAD",
            "--cache",
            "unused",
            "--no-cache",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!fixture.path().join("unused").exists());

    if PathCase::probe(fixture.path()) == PathCase::Insensitive {
        let output = command(&fixture)
            .args([
                "check",
                "--release-history",
                "HEAD",
                "--cache",
                "PACKAGES/DEMO",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fixture.read("packages/demo/src/lib.rs"), original);
    }

    let evidence = TempDir::new().unwrap();
    let output = command(&fixture)
        .args(["report", "--release-history", "HEAD", "--cache"])
        .arg(evidence.path())
        .arg("--out-dir")
        .arg(evidence.path().join("report"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!evidence.path().join("report").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes captured-input and cache admission boundaries"
)]
fn cache_admission_protects_absent_and_recursive_source_inputs() {
    let fixture = seeded_package();
    fixture.write_workspace("exclude = ['vendor/helper', 'vendor/leaf']");
    fixture.write(".gitignore", "/vendor/\n");
    fixture.write(
        "packages/demo/Cargo.toml",
        &format!(
            "{}\n[dependencies]\nhelper = {{ path = '../../vendor/helper' }}\n",
            fixture.read("packages/demo/Cargo.toml")
        ),
    );
    fixture.write(
        "vendor/helper/Cargo.toml",
        "[package]\nname='helper'\nversion='0.1.0'\n\
         [dependencies]\nleaf={path='../leaf'}\n",
    );
    fixture.write("vendor/helper/src/lib.rs", "");
    fixture.write(
        "vendor/leaf/Cargo.toml",
        "[package]\nname='leaf'\nversion='0.1.0'\n",
    );
    fixture.write("vendor/leaf/src/lib.rs", "");
    fixture.write(
        "unselected/Cargo.toml",
        "[package]\nname='unselected'\nversion='0.1.0'\n",
    );
    fixture.write("unselected/src/lib.rs", "");
    fixture.commit("ignored transitive path dependencies");
    let evidence = TempDir::new().unwrap();
    report(
        &fixture,
        &evidence.path().join("baseline"),
        &evidence.path().join("baseline.trace"),
        &["--no-cache"],
    );
    let inputs = Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap();
    for path in [
        "packages/demo/build.rs",
        "Cargo.lock",
        ".cargo/config",
        ".cargo/config.toml",
        "vendor/helper",
        "vendor/leaf",
        "vendor/leaf/src/cache",
        "vendor/leaf/build.rs",
        "unselected/build.rs",
    ] {
        let output = command(&fixture)
            .args(["check", "--release-history", "HEAD", "--cache", path])
            .output()
            .unwrap();
        assert!(!output.status.success(), "{path}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("overlaps protected"),
            "{path}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        inputs.verify(&fixture.manifest(), None).unwrap();
    }
    assert!(!fixture.path().join("vendor/leaf/build.rs").exists());
    assert!(!fixture.path().join("unselected/build.rs").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "runs classification and strict capture with unused missing paths"
)]
fn unavailable_extra_inventory_disables_storage_without_failing_classification() {
    let fixture = seeded_package();
    fixture.write_workspace("[workspace.dependencies]\nunused = { path = 'unavailable' }\n");
    fixture.commit("unused missing path declaration");
    let evidence = TempDir::new().unwrap();
    for (name, options) in [("baseline", &["--no-cache"][..]), ("cached", &[][..])] {
        success(
            command(&fixture)
                .args(["check", "--release-history", "HEAD"])
                .args(options)
                .output()
                .unwrap(),
        );
        report(
            &fixture,
            &evidence.path().join(name),
            &evidence.path().join(format!("{name}.trace")),
            options,
        );
    }
    assert_eq!(
        fs::read(evidence.path().join("baseline/report.json")).unwrap(),
        fs::read(evidence.path().join("cached/report.json")).unwrap()
    );
    assert!(
        !fixture
            .path()
            .join("target/cargo-release-plan/cache")
            .exists()
    );
    Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes classification with an obstructed unrelated tracked path"
)]
fn unresolved_extra_inventory_disables_storage_without_failing_classification() {
    let fixture = seeded_package();
    fixture.write("docs/page.md", "unrelated documentation");
    fixture.commit("documentation");
    let inputs = Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap();
    fs::remove_file(fixture.path().join("docs/page.md")).unwrap();
    fs::remove_dir(fixture.path().join("docs")).unwrap();
    fixture.write("docs", "not a directory");
    let evidence = TempDir::new().unwrap();
    let baseline = evidence.path().join("baseline");
    report(
        &fixture,
        &baseline,
        &evidence.path().join("baseline.trace"),
        &["--no-cache"],
    );
    let cached = evidence.path().join("cached");
    let output = report(
        &fixture,
        &cached,
        &evidence.path().join("cached.trace"),
        &[],
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("storage disabled"));
    assert_reports_equal(&baseline, &cached);
    assert!(
        !fixture
            .path()
            .join("target/cargo-release-plan/cache")
            .exists()
    );
    inputs.verify(&fixture.manifest(), None).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "executes Git with external object and index locations")]
fn external_git_object_and_index_locations_are_protected() {
    let fixture = seeded_package();
    let external = TempDir::new().unwrap();
    let objects = external.path().join("objects");
    let index = external.path().join("index");
    let hooks = external.path().join("hooks");
    let alternates = external.path().join("alternate objects");
    fs::create_dir_all(&alternates).unwrap();
    fixture.git(&["config", "core.hooksPath", hooks.to_str().unwrap()]);
    fs::rename(fixture.path().join(".git/objects"), &objects).unwrap();
    fs::rename(fixture.path().join(".git/index"), &index).unwrap();
    for path in [&objects, &index, &hooks, &alternates] {
        let output = command(&fixture)
            .args(["check", "--release-history", "HEAD", "--cache"])
            .arg(path)
            .env("GIT_OBJECT_DIRECTORY", &objects)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alternates)
            .output()
            .unwrap();
        assert!(!output.status.success(), "{}", path.display());
        assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps protected"));
    }
    assert!(!objects.join(".gitignore").exists());
    success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD", "--no-cache"])
            .env("GIT_OBJECT_DIRECTORY", &objects)
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &alternates)
            .output()
            .unwrap(),
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "checks Cargo autodiscovery and untracked target visibility"
)]
fn untracked_autodiscovery_targets_cannot_be_cache_storage() {
    let fixture = seeded_package();
    for directory in ["examples", "tests", "benches"] {
        let target = format!("packages/demo/{directory}/demo.rs");
        fixture.write(&target, "fn main() {}\n");
        let output = command(&fixture)
            .args(["check", "--release-history", "HEAD", "--cache"])
            .arg(fixture.path().join("packages/demo").join(directory))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("overlaps protected"));
        assert!(
            fixture
                .git(&["status", "--porcelain", "--untracked-files=all"])
                .contains(&target)
        );
        assert_eq!(fixture.read(&target), "fn main() {}\n");
    }
}

#[test]
#[cfg_attr(miri, ignore = "executes compatibility with a closed diagnostic pipe")]
fn cache_advisories_do_not_fail_compatibility_with_closed_stderr() {
    let fixture = seeded_package();
    fixture.write(
        "packages/demo/Cargo.toml",
        &format!(
            "{}\n[package.metadata.release-plan]\nprivate-api = true\n",
            fixture.read("packages/demo/Cargo.toml")
        ),
    );
    fixture.commit("private application contract");
    let evidence = TempDir::new().unwrap();
    let unavailable = evidence.path().join("obstruction");
    fs::write(&unavailable, "not a directory").unwrap();
    let corrupt = evidence.path().join("corrupt");
    report(
        &fixture,
        &evidence.path().join("seed"),
        &evidence.path().join("seed.trace"),
        &["--cache", corrupt.to_str().unwrap()],
    );
    for entry in entries(&corrupt, "git-trees") {
        fs::write(entry, "{").unwrap();
    }
    for (index, storage) in [unavailable.join("cache"), corrupt].into_iter().enumerate() {
        let (reader, writer) = io::pipe().unwrap();
        drop(reader);
        success(
            command(&fixture)
                .args([
                    "check-compatibility",
                    "--release-history",
                    "HEAD",
                    "--cache",
                ])
                .arg(storage)
                .arg("--output")
                .arg(evidence.path().join(format!("compatibility-{index}")))
                .stderr(writer)
                .output()
                .unwrap(),
        );
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "probes native case rules and executes output admission"
)]
fn cache_admission_uses_case_rules_for_missing_destination_components() {
    let fixture = seeded_package();
    let evidence = TempDir::new().unwrap();
    // A known ordinary entry makes the fixture's independent read-only case probe decisive.
    fs::write(evidence.path().join("case-probe"), "").unwrap();
    let case = PathCase::probe(evidence.path());
    for (index, (cache, output)) in [
        ("CACHE", "cache/report"),
        ("CACHE/nested", "cache"),
        ("missing/CACHE", "missing/cache/report"),
    ]
    .into_iter()
    .enumerate()
    {
        let destination = evidence.path().join(index.to_string());
        fs::create_dir_all(&destination).unwrap();
        let output = command(&fixture)
            .args(["report", "--release-history", "HEAD", "--cache"])
            .arg(destination.join(cache))
            .arg("--out-dir")
            .arg(destination.join(output))
            .output()
            .unwrap();
        assert_eq!(output.status.success(), case == PathCase::Sensitive);
    }
}

#[test]
#[cfg_attr(miri, ignore = "executes commands with obstructed cache locations")]
fn cache_location_failures_leave_source_and_reports_usable() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let obstruction = evidence.path().join("obstruction");
    fs::write(&obstruction, "not a directory").unwrap();
    let baseline = evidence.path().join("baseline");
    report(
        &fixture,
        &baseline,
        &evidence.path().join("baseline.trace"),
        &["--no-cache"],
    );
    let source = Inputs::capture(&fixture.manifest(), Some("HEAD")).unwrap();
    fixture.write("target/cargo-release-plan", "not a directory");
    for (name, options) in [
        (
            "explicit",
            vec!["--cache".into(), obstruction.join("cache").into_os_string()],
        ),
        ("default", Vec::new()),
    ] {
        let destination = evidence.path().join(name);
        let output = success(
            command(&fixture)
                .args(["report", "--release-history", "HEAD", "--out-dir"])
                .arg(&destination)
                .args(options)
                .output()
                .unwrap(),
        );
        assert!(!output.stderr.is_empty());
        assert_reports_equal(&baseline, &destination);
        source.verify(&fixture.manifest(), None).unwrap();
    }
    assert_eq!(fs::read_to_string(obstruction).unwrap(), "not a directory");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "examines native cache write failures and existing files"
)]
fn storage_failures_are_advisory_and_existing_ignore_rules_are_preserved() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let baseline = evidence.path().join("baseline");
    report(
        &fixture,
        &baseline,
        &evidence.path().join("baseline.trace"),
        &["--no-cache"],
    );
    let storage = evidence.path().join("cache");
    fs::create_dir_all(&storage).unwrap();
    let ignore = storage.join(".gitignore");
    fs::write(&ignore, "# caller-owned rules\n").unwrap();
    // A regular file at the subject directory is a deterministic I/O failure on every platform.
    let obstruction = storage.join("git-trees");
    fs::write(&obstruction, "not a directory").unwrap();
    let output = report(
        &fixture,
        &evidence.path().join("unavailable"),
        &evidence.path().join("unavailable.trace"),
        &["--cache", storage.to_str().unwrap()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr
            .lines()
            .filter(|line| line.starts_with("[release-plan] cache entry"))
            .count(),
        1
    );
    assert!(stderr.contains("continuing with fresh observations"));
    assert_reports_equal(&baseline, &evidence.path().join("unavailable"));
    fs::remove_file(obstruction).unwrap();
    report(
        &fixture,
        &evidence.path().join("repaired"),
        &evidence.path().join("repaired.trace"),
        &["--cache", storage.to_str().unwrap()],
    );
    assert_eq!(
        fs::read_to_string(ignore).unwrap(),
        "# caller-owned rules\n"
    );
    assert!(!entries(&storage, "git-trees").is_empty());
}

#[test]
#[cfg_attr(miri, ignore = "executes concurrent native cache publication")]
fn malformed_entries_are_diagnosed_and_concurrent_publishers_leave_complete_entries() {
    // Concurrent native startup needs a last-chance budget well above ordinary completion;
    // the test never waits for this deadline to assert a failure.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = seeded_package();
        fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let cache_argument = storage.to_str().unwrap();
        let baseline = evidence.path().join("baseline");
        report(
            &fixture,
            &baseline,
            &evidence.path().join("baseline.trace"),
            &["--no-cache"],
        );
        report(
            &fixture,
            &evidence.path().join("first"),
            &evidence.path().join("first.trace"),
            &["--cache", cache_argument],
        );
        for subject in ["git-trees", "git-parent-headers"] {
            for path in entries(&storage, subject) {
                fs::write(path, "{").unwrap();
            }
        }
        let output = report(
            &fixture,
            &evidence.path().join("repaired"),
            &evidence.path().join("repaired.trace"),
            &["--cache", cache_argument],
        );
        assert!(String::from_utf8_lossy(&output.stderr).contains("corrupt cache"));
        assert_reports_equal(&baseline, &evidence.path().join("repaired"));
        assert_eq!(
            acquisitions(&evidence.path().join("repaired.trace")),
            (1, 1)
        );
        fs::remove_dir_all(&storage).unwrap();
        thread::scope(|scope| {
            let mut writers = Vec::new();
            for index in 0..4 {
                let fixture = &fixture;
                let evidence = evidence.path();
                writers.push(scope.spawn(move || {
                    report(
                        fixture,
                        &evidence.join(format!("writer-{index}")),
                        &evidence.join(format!("writer-{index}.trace")),
                        &["--cache", cache_argument],
                    );
                }));
            }
            for writer in writers {
                writer.join().unwrap();
            }
        });
        report(
            &fixture,
            &evidence.path().join("final"),
            &evidence.path().join("final.trace"),
            &["--cache", cache_argument],
        );
        assert_eq!(acquisitions(&evidence.path().join("final.trace")), (0, 0));
        assert_eq!(entries(&storage, "git-trees").len(), 1);
        assert_eq!(entries(&storage, "git-parent-headers").len(), 1);
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes preparation, prospective resolution and source verification"
)]
fn preview_keeps_original_cache_location_and_cache_removal_does_not_invalidate_evidence() {
    let fixture = seeded_package();
    fixture.write(
        "packages/demo/Cargo.toml",
        &format!(
            "{}\n[package.metadata.release-plan]\nprivate-api = true\n",
            fixture.read("packages/demo/Cargo.toml")
        ),
    );
    fixture.write(
        ".cargo/config.toml",
        "[build]\ntarget-dir = 'configured-target'\n",
    );
    fixture.commit("target configuration");
    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    let evidence = TempDir::new().unwrap();
    let prepared = evidence.path().join("prepared");
    success(
        command(&fixture)
            .args(["prepare", "--release-history", "HEAD", "--output"])
            .arg(&prepared)
            .output()
            .unwrap(),
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
    let result = success(
        command(&fixture)
            .args(["preview", "--verbose", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--plan")
            .arg(&plan)
            .arg("--output")
            .arg(&preview)
            .env("GIT_TRACE", &trace)
            .output()
            .unwrap(),
    );
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("reusing classification decisions from memory")
    );
    assert_eq!(acquisitions(&trace), (0, 0));
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
    let relocated = evidence.path().join("relocated-preview");
    let result = success(
        command(&fixture)
            .args(["preview", "--verbose", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--plan")
            .arg(&plan)
            .arg("--output")
            .arg(&relocated)
            .output()
            .unwrap(),
    );
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(
        diagnostics.contains("reusing classification decisions from storage"),
        "{diagnostics}"
    );
    assert!(
        !diagnostics.contains("computed classification decisions"),
        "{diagnostics}"
    );
    assert_eq!(
        fs::read(preview.join("report.json")).unwrap(),
        fs::read(relocated.join("report.json")).unwrap()
    );
    let relocated_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(relocated.join("plan.json")).unwrap()).unwrap();
    for path in ["/increments", "/resolved/files", "/resolved/final_digest"] {
        assert_eq!(
            resolved.pointer(path).unwrap(),
            relocated_plan.pointer(path).unwrap()
        );
    }
    assert_ne!(
        resolved
            .pointer("/resolved/evidence_manifest_path")
            .unwrap(),
        relocated_plan
            .pointer("/resolved/evidence_manifest_path")
            .unwrap()
    );
    let uncached = evidence.path().join("uncached-preview");
    success(
        command(&fixture)
            .args(["preview", "--no-cache", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--plan")
            .arg(&plan)
            .arg("--output")
            .arg(&uncached)
            .output()
            .unwrap(),
    );
    assert_eq!(
        fs::read(preview.join("report.json")).unwrap(),
        fs::read(uncached.join("report.json")).unwrap()
    );
    let uncached_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(uncached.join("plan.json")).unwrap()).unwrap();
    for path in ["/resolved/files", "/resolved/final_digest"] {
        assert_eq!(
            resolved.pointer(path).unwrap(),
            uncached_plan.pointer(path).unwrap()
        );
    }
    for (name, cache) in [
        ("candidate-root", manifest.parent().unwrap().to_path_buf()),
        (
            "candidate-child",
            manifest.parent().unwrap().join("unused-cache"),
        ),
    ] {
        let output = command(&fixture)
            .args(["check-compatibility", "--plan"])
            .arg(preview.join("plan.json"))
            .arg("--cache")
            .arg(&cache)
            .arg("--output")
            .arg(evidence.path().join(name))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(!cache.join("git-trees").exists());
    }
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
    assert_eq!(acquisitions(&compatibility_trace), (0, 0));
    assert!(
        !manifest
            .parent()
            .unwrap()
            .join("configured-target")
            .join("cargo-release-plan")
            .join("cache")
            .exists()
    );
    let storage = fixture
        .path()
        .join("configured-target")
        .join("cargo-release-plan")
        .join("cache");
    fs::remove_dir_all(&storage).unwrap();
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
    assert!(!storage.exists());

    // Retained evidence still requires valid original inputs, even with storage disabled.
    fixture.write_workspace("[workspace.dependencies]\nunused = { path = 'unavailable' }\n");
    for (name, options) in [
        ("frozen", &[][..]),
        ("frozen-disabled", &["--no-cache"][..]),
    ] {
        let output = command(&fixture)
            .args(["check-compatibility", "--plan"])
            .arg(preview.join("plan.json"))
            .arg("--output")
            .arg(evidence.path().join(name))
            .args(options)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("stale"));
    }
    assert!(!storage.exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "executes independent application processes and changes native inputs"
)]
fn decision_entries_recompute_changed_inputs_and_reject_incompatible_or_corrupt_payloads() {
    let fixture = seeded_package();
    let evidence = TempDir::new().unwrap();
    let storage = evidence.path().join("cache");
    let options = ["--cache", storage.to_str().unwrap(), "--verbose"];
    let run = |name: &str| {
        report(
            &fixture,
            &evidence.path().join(name),
            &evidence.path().join(format!("{name}.trace")),
            &options,
        )
    };
    run("initial");
    let check = success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD"])
            .args(options)
            .output()
            .unwrap(),
    );
    assert!(
        String::from_utf8_lossy(&check.stderr)
            .contains("reusing classification decisions from storage")
    );
    let uncached_check = success(
        command(&fixture)
            .args(["check", "--release-history", "HEAD", "--no-cache"])
            .output()
            .unwrap(),
    );
    assert_eq!(check.stdout, uncached_check.stdout);

    let path = entries(&storage, "classification-decisions").pop().unwrap();
    let original: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    let mut incompatible = original.clone();
    *incompatible.get_mut("revision").unwrap() =
        json!(original.get("revision").unwrap().as_u64().unwrap() + 1);
    fs::write(&path, serde_json::to_vec(&incompatible).unwrap()).unwrap();
    let result = run("incompatible");
    assert!(String::from_utf8_lossy(&result.stderr).contains("computed classification decisions"));
    let mut corrupt = original;
    *corrupt.get_mut("checksum").unwrap() = "damaged".into();
    fs::write(&path, serde_json::to_vec(&corrupt).unwrap()).unwrap();
    let result = run("corrupt");
    assert!(String::from_utf8_lossy(&result.stderr).contains("corrupt cache"));
    assert!(String::from_utf8_lossy(&result.stderr).contains("computed classification decisions"));
    assert_eq!(
        fs::read(evidence.path().join("initial/report.json")).unwrap(),
        fs::read(evidence.path().join("corrupt/report.json")).unwrap()
    );

    fixture.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    let result = run("content");
    assert!(String::from_utf8_lossy(&result.stderr).contains("computed classification decisions"));
    fixture.write("packages/demo/advisory.txt", "untracked");
    let result = run("advisory");
    assert!(String::from_utf8_lossy(&result.stderr).contains("computed classification decisions"));
    fixture.git(&["update-index", "--chmod=+x", "packages/demo/src/lib.rs"]);
    // Unix observes the filesystem overlay; Windows preserves the index's executable bit.
    #[cfg(unix)]
    {
        let path = fixture.path().join("packages/demo/src/lib.rs");
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions).unwrap();
    }
    let result = run("mode");
    assert!(String::from_utf8_lossy(&result.stderr).contains("computed classification decisions"));
    report(
        &fixture,
        &evidence.path().join("fresh"),
        &evidence.path().join("fresh.trace"),
        &["--no-cache"],
    );
    assert_eq!(
        fs::read(evidence.path().join("mode/report.json")).unwrap(),
        fs::read(evidence.path().join("fresh/report.json")).unwrap()
    );
    assert_eq!(
        fs::read(evidence.path().join("mode/diffs/demo.patch")).unwrap(),
        fs::read(evidence.path().join("fresh/diffs/demo.patch")).unwrap()
    );

    fixture.write("packages/demo/Cargo.toml", "not a manifest");
    let failed = command(&fixture)
        .args(["check", "--release-history", "HEAD"])
        .args(options)
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert!(!String::from_utf8_lossy(&failed.stderr).contains("reusing classification decisions"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "commits unrelated source while reusing decisions in separate processes"
)]
fn decision_hit_reconstructs_current_head_instead_of_returning_a_cached_classification() {
    let fixture = seeded_package();
    let history = fixture.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let evidence = TempDir::new().unwrap();
    let run = |name: &str| {
        success(
            command(&fixture)
                .args([
                    "report",
                    "--release-history",
                    &history,
                    "--verbose",
                    "--out-dir",
                ])
                .arg(evidence.path().join(name))
                .output()
                .unwrap(),
        )
    };
    run("before");
    fixture.write("unrelated.txt", "outside released package scope");
    fixture.commit("unrelated committed file");
    let head = fixture.git(&["rev-parse", "HEAD"]).trim().to_owned();
    let result = run("after");
    let diagnostics = String::from_utf8_lossy(&result.stderr);
    assert!(
        diagnostics.contains("reusing classification decisions from storage"),
        "{diagnostics}"
    );
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.path().join("before/report.json")).unwrap())
            .unwrap();
    let after: serde_json::Value =
        serde_json::from_slice(&fs::read(evidence.path().join("after/report.json")).unwrap())
            .unwrap();
    assert_ne!(history, head);
    assert_eq!(after.get("head").unwrap(), &head);
    assert_eq!(
        before.get("packages").unwrap(),
        after.get("packages").unwrap()
    );
}
