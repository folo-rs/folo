//! Executable cache selection, persistence and evidence independence over hermetic repositories.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;
use std::{fs, thread};

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

fn entries(directory: &Path, subject: &str) -> Vec<PathBuf> {
    fs::read_dir(directory.join(subject))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect()
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
        report(&fixture, output, &trace, options);
        assert_eq!(
            acquisitions(&trace),
            if name == "warm" { (0, 0) } else { (1, 1) }
        );
    }
    for output in [&warm, &disabled] {
        assert_eq!(
            fs::read(cold.join("report.json")).unwrap(),
            fs::read(output.join("report.json")).unwrap()
        );
        for entry in fs::read_dir(cold.join("diffs")).unwrap() {
            let entry = entry.unwrap();
            assert_eq!(
                fs::read(entry.path()).unwrap(),
                fs::read(output.join("diffs").join(entry.file_name())).unwrap()
            );
        }
    }
    original.verify(&fixture.manifest(), None).unwrap();
    let storage = fixture
        .path()
        .join("target")
        .join("cargo-release-plan")
        .join("cache");
    assert_eq!(entries(&storage, "git-trees").len(), 1);
    assert_eq!(entries(&storage, "git-parent-headers").len(), 1);
    assert!(
        !fixture
            .git(&["status", "--porcelain", "--untracked-files=all"])
            .contains("cache")
    );
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
    for path in ["packages/demo/src/cache", "packages/demo", "Cargo.toml"] {
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
    ignore = "examines native cache write failures and existing files"
)]
fn storage_failures_are_advisory_and_existing_ignore_rules_are_preserved() {
    let fixture = seeded_package();
    let evidence = TempDir::new().unwrap();
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
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let cache_argument = storage.to_str().unwrap();
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
            .output()
            .unwrap(),
    );
    assert!(!storage.exists());
}
