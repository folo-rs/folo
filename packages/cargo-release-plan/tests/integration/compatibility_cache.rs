//! Cross-invocation decision reuse must not extend compatibility source admission.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::cache::entries;
use crate::compatibility::{
    CHECKER_WATCHDOG, anticipated_parent, checker_command, configure_git_shim, read_outcome,
};
use crate::fixture::{Fixture, write_package};

/// Owns a prepared child and its retained candidate over an unpublished parent API.
///
/// Every operation starts the real CLI; only the external checker is a local protocol fixture.
struct Assessment {
    fixture: Fixture,
    evidence: TempDir,
    parent: String,
    mode: &'static str,
    storage: PathBuf,
}

impl Assessment {
    fn new(mode: &'static str) -> Self {
        let (fixture, history, parent) = anticipated_parent();
        write_package(&fixture, "library", "1.1.1", "");
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        fixture.write(
            ".cargo/config.toml",
            "[build]\ntarget-dir = 'configured-target'\n",
        );
        fixture.commit("child removes parent API");
        fixture.git(&["branch", "release-history", &history]);
        let evidence = TempDir::new().unwrap();
        let storage = match mode {
            "default" | "disabled" => fixture
                .path()
                .join("configured-target/cargo-release-plan/cache"),
            "environment" => evidence.path().join("build/cargo-release-plan/cache"),
            "relative" => evidence.path().join("relative-cache"),
            "absolute" => evidence.path().join("absolute-cache"),
            _ => panic!("unknown cache fixture mode"),
        };
        let assessment = Self {
            fixture,
            evidence,
            parent,
            mode,
            storage,
        };
        success(
            assessment
                .command("prepare")
                .args([
                    "--release-history",
                    "release-history",
                    "--merge-target",
                    "anticipated-parent",
                    "--output",
                ])
                .arg(assessment.path("prepared"))
                .output()
                .unwrap(),
        );
        let proposal = assessment.path("proposal.json");
        fs::write(
            &proposal,
            serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "increments": []
            }))
            .unwrap(),
        )
        .unwrap();
        success(
            assessment
                .command("preview")
                .arg("--prepared")
                .arg(assessment.path("prepared/prepared.json"))
                .arg("--plan")
                .arg(proposal)
                .arg("--output")
                .arg(assessment.path("preview"))
                .output()
                .unwrap(),
        );
        assessment
    }

    fn path(&self, path: &str) -> PathBuf {
        self.evidence.path().join(path)
    }

    fn command(&self, operation: &str) -> Command {
        let mut command = checker_command();
        command
            .current_dir(self.evidence.path())
            .env_remove("CARGO_TARGET_DIR")
            .args([operation, "--verbose", "--manifest-path"])
            .arg(self.fixture.manifest());
        match self.mode {
            "environment" => {
                command.env("CARGO_TARGET_DIR", self.path("build"));
            }
            "relative" => {
                command.args(["--cache", "relative-cache"]);
            }
            "absolute" => {
                command.arg("--cache").arg(&self.storage);
            }
            "disabled" => {
                command.arg("--no-cache");
            }
            "default" => {}
            _ => panic!("unknown cache fixture mode"),
        }
        command
    }

    fn check(&self, name: &str) -> Command {
        self.check_mode(name, "--plan", "preview/plan.json")
    }

    fn check_mode(&self, name: &str, option: &str, artifact: &str) -> Command {
        let mut command = self.command("check-compatibility");
        command
            .arg(option)
            .arg(self.path(artifact))
            .arg("--output")
            .arg(self.path(name))
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &self.parent)
            .env("CRP_FIXTURE_CALLS", self.path(&format!("{name}.calls")))
            .env(
                "CRP_FIXTURE_SOURCE",
                self.fixture.path().join("packages/library/src/lib.rs"),
            );
        command
    }
}

fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

fn reused(output: &Output) {
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains("reusing classification decisions from storage"),
        "{diagnostics}"
    );
    assert!(!diagnostics.contains("computed classification decisions"));
}

fn candidate(assessment: &Assessment) -> PathBuf {
    let plan: Value =
        serde_json::from_slice(&fs::read(assessment.path("preview/plan.json")).unwrap()).unwrap();
    Path::new(
        plan.pointer("/resolved/evidence_manifest_path")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .parent()
    .unwrap()
    .to_path_buf()
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Admits an explicit cache override against retained Git evidence"
)]
fn cache_override_cannot_write_into_the_retained_candidate() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        let storage = candidate(&assessment).join("packages/library/src/cache");
        let result = assessment
            .check("overlap")
            .arg("--cache")
            .arg(&storage)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!storage.exists());
        assert!(!assessment.path("overlap/report.json").exists());
        assert!(!assessment.path("overlap/compatibility.json").exists());
        assert!(!assessment.path("overlap.calls").exists());
        reused(&success(assessment.check("intact").output().unwrap()));
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs cached preview admission and a native checker mutation"
)]
fn preview_cache_hit_rechecks_source_and_candidate_around_checker() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        for (subject, source) in [
            (
                "original",
                assessment
                    .fixture
                    .path()
                    .join("packages/library/src/lib.rs"),
            ),
            (
                "candidate",
                candidate(&assessment).join("packages/library/src/lib.rs"),
            ),
        ] {
            let original = fs::read(&source).unwrap();
            for phase in ["canary", "comparison"] {
                let name = format!("{subject}-{phase}");
                let output = assessment
                    .check(&name)
                    .env("CRP_FIXTURE_MUTATION_PHASE", phase)
                    .env("CRP_FIXTURE_MUTATION_PATH", &source)
                    .output()
                    .unwrap();
                reused(&output);
                assert!(!output.status.success());
                // Canary drift must stop before baseline assessment, not just checker execution.
                assert_eq!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("Comparing library against baseline"),
                    phase == "comparison"
                );
                let outcome = read_outcome(&assessment.path(&name));
                assert_eq!(outcome.get("completed").unwrap(), false);
                assert_eq!(outcome.get("findings").unwrap(), phase == "comparison");
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{name}.calls"))).unwrap(),
                    if phase == "canary" {
                        "version\ncanary\n"
                    } else {
                        "version\ncanary\ncomparison\n"
                    }
                );
                assert!(
                    fs::read_to_string(&source)
                        .unwrap()
                        .contains("changed_during")
                );
                fs::write(&source, &original).unwrap();
            }
        }
        assert!(candidate(&assessment).join("Cargo.toml").is_file());
    });
}

fn same_evidence(expected: &Path, actual: &Path) {
    assert_eq!(
        fs::read(expected.join("report.json")).unwrap(),
        fs::read(actual.join("report.json")).unwrap()
    );
    let mut expected_outcome = read_outcome(expected);
    let mut actual_outcome = read_outcome(actual);
    for outcome in [&mut expected_outcome, &mut actual_outcome] {
        // Each invocation necessarily owns a different output report path.
        outcome.as_object_mut().unwrap().remove("report").unwrap();
    }
    assert_eq!(expected_outcome, actual_outcome);
    assert_eq!(
        fs::read(expected.join("semver-checks.log")).unwrap(),
        fs::read(actual.join("semver-checks.log")).unwrap()
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
#[cfg_attr(
    miri,
    ignore = "Uses separate CLI processes and retained Cargo/Git workspaces"
)]
fn prepared_and_preview_checks_use_default_cache() {
    prepared_and_preview_checks("default");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Cargo target-directory environment and a native checker"
)]
fn prepared_and_preview_checks_use_environment_cache() {
    prepared_and_preview_checks("environment");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses invocation-relative storage and retained native workspaces"
)]
fn prepared_and_preview_checks_use_relative_cache() {
    prepared_and_preview_checks("relative");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses explicit absolute storage and retained native workspaces"
)]
fn prepared_and_preview_checks_use_absolute_cache() {
    prepared_and_preview_checks("absolute");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Executes uncached Git/Cargo acquisition and a native checker"
)]
fn prepared_and_preview_checks_bypass_cache() {
    prepared_and_preview_checks("disabled");
}

fn prepared_and_preview_checks(mode: &'static str) {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, move || {
        let assessment = Assessment::new(mode);
        // A valid report from a different history has no comparison targets. It must not
        // substitute for the source/history bound by either selected evidence artifact.
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .args([
                    "report",
                    "--no-cache",
                    "--release-history",
                    "HEAD",
                    "--out-dir",
                ])
                .arg(assessment.path("unrelated"))
                .output()
                .unwrap(),
        );
        let unrelated = fs::read(assessment.path("unrelated/report.json")).unwrap();
        let unrelated_report: Value = serde_json::from_slice(&unrelated).unwrap();
        assert_eq!(
            unrelated_report.pointer("/packages/0/status").unwrap(),
            "unchanged"
        );
        for (name, option, artifact) in [
            ("prepared", "--prepared", "prepared/prepared.json"),
            ("preview", "--plan", "preview/plan.json"),
        ] {
            fs::write(assessment.path(&format!("{name}/report.json")), &unrelated).unwrap();
            let cached_name = format!("{name}-cached");
            let output = success(
                assessment
                    .check_mode(&cached_name, option, artifact)
                    .output()
                    .unwrap(),
            );
            if mode != "disabled" {
                reused(&output);
                assert!(assessment.storage.join("classification-decisions").is_dir());
            } else {
                assert!(
                    !String::from_utf8_lossy(&output.stderr)
                        .contains("reusing classification decisions")
                );
                assert!(!assessment.storage.exists());
            }

            let uncached_name = format!("{name}-uncached");
            let mut uncached = checker_command();
            uncached
                .current_dir(assessment.evidence.path())
                .args(["check-compatibility", "--no-cache", "--manifest-path"])
                .arg(assessment.fixture.manifest())
                .arg(option)
                .arg(assessment.path(artifact))
                .arg("--output")
                .arg(assessment.path(&uncached_name))
                .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
                .env("CRP_EXPECTED_PARENT", &assessment.parent)
                .env(
                    "CRP_FIXTURE_CALLS",
                    assessment.path(&format!("{uncached_name}.calls")),
                )
                .env(
                    "CRP_FIXTURE_SOURCE",
                    assessment
                        .fixture
                        .path()
                        .join("packages/library/src/lib.rs"),
                );
            success(uncached.output().unwrap());
            same_evidence(
                &assessment.path(&cached_name),
                &assessment.path(&uncached_name),
            );
            for invocation in [&cached_name, &uncached_name] {
                assert_eq!(
                    fs::read_to_string(assessment.path(&format!("{invocation}.calls"))).unwrap(),
                    "version\ncanary\ncomparison\n"
                );
            }
            let outcome = read_outcome(&assessment.path(&cached_name));
            assert_eq!(outcome.get("completed").unwrap(), true);
            assert_eq!(outcome.get("findings").unwrap(), true);
            assert_eq!(
                outcome.pointer("/packages/0/baseline_version").unwrap(),
                "1.1.0"
            );
        }
        assert!(
            !candidate(&assessment)
                .join("configured-target/cargo-release-plan/cache")
                .exists()
        );
        assert_eq!(
            assessment
                .fixture
                .git(&["worktree", "list", "--porcelain"])
                .matches("worktree ")
                .count(),
            1
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Replaces native cache entries between independent CLI invocations"
)]
fn no_target_checks_admit_source_with_missing_disabled_or_invalid_cache() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
        fixture.commit("unchanged public library");
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let prepared = evidence.path().join("prepared");
        success(
            checker_command()
                .current_dir(fixture.path())
                .args(["prepare", "--release-history", "HEAD", "--cache"])
                .arg(&storage)
                .arg("--output")
                .arg(&prepared)
                .output()
                .unwrap(),
        );
        let expected = fs::read(prepared.join("report.json")).unwrap();
        let mut baseline: Option<PathBuf> = None;
        for state in [
            "warm",
            "corrupt",
            "format",
            "revision",
            "producer",
            "deleted",
            "unavailable",
            "disabled",
        ] {
            if state == "deleted" || state == "unavailable" {
                fs::remove_dir_all(&storage).unwrap();
                if state == "unavailable" {
                    fs::create_dir_all(&storage).unwrap();
                    fs::write(storage.join("classification-decisions"), "not a directory").unwrap();
                }
            } else if ["corrupt", "format", "revision", "producer"].contains(&state) {
                for path in entries(&storage, "classification-decisions") {
                    let mut entry: Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    match state {
                        "corrupt" => {
                            *entry.get_mut("checksum").unwrap() = json!("invalid integrity");
                        }
                        "format" | "revision" => *entry.get_mut(state).unwrap() = json!(u32::MAX),
                        "producer" => {
                            let key: String =
                                serde_json::from_str(entry.get("key").unwrap().as_str().unwrap())
                                    .unwrap();
                            let mut key: Value = serde_json::from_str(&key).unwrap();
                            *key.get_mut(0).unwrap() = json!("incompatible-producer");
                            *entry.get_mut("key").unwrap() =
                                json!(serde_json::to_string(&key.to_string()).unwrap());
                        }
                        _ => unreachable!(),
                    }
                    fs::write(path, serde_json::to_vec(&entry).unwrap()).unwrap();
                }
            }
            let output = evidence.path().join(state);
            let calls = evidence.path().join(format!("{state}.calls"));
            let mut command = checker_command();
            command
                .current_dir(fixture.path())
                .args(["check-compatibility", "--verbose", "--prepared"])
                .arg(prepared.join("prepared.json"))
                .arg("--output")
                .arg(&output)
                .env("CRP_FIXTURE_CALLS", &calls)
                .env("CRP_FIXTURE_SCENARIO", "identity-failure");
            if state == "disabled" {
                command.arg("--no-cache");
            } else {
                command.arg("--cache").arg(&storage);
            }
            let result = success(command.output().unwrap());
            let diagnostics = String::from_utf8_lossy(&result.stderr);
            if state == "warm" {
                reused(&result);
            } else {
                assert!(!diagnostics.contains("reusing classification decisions"));
                if state != "disabled" {
                    assert!(
                        diagnostics.contains("computed classification decisions"),
                        "{diagnostics}"
                    );
                }
            }
            if state == "corrupt" || state == "unavailable" {
                assert!(diagnostics.contains("continuing with fresh observations"));
            }
            assert!(!calls.exists());
            assert_eq!(fs::read(output.join("report.json")).unwrap(), expected);
            let outcome = read_outcome(&output);
            assert_eq!(outcome.get("packages").unwrap(), &json!([]));
            assert_eq!(outcome.get("completed").unwrap(), true);
            if let Some(baseline) = &baseline {
                same_evidence(baseline, &output);
            } else {
                baseline = Some(output);
            }
            if state == "unavailable" {
                fs::remove_dir_all(&storage).unwrap();
            }
        }
        assert!(!storage.exists());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Shares decisions between distinct real Git/Cargo repositories"
)]
fn shared_decisions_keep_current_provenance_and_changed_inputs_recompute() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(
            &fixture,
            "library",
            "1.0.0",
            "[package.metadata.release-plan]\nprivate-api = true\n",
        );
        fixture.commit("private library history");
        let history = fixture.sha("HEAD");
        fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let prepared = evidence.path().join("prepared");
        success(
            checker_command()
                .current_dir(fixture.path())
                .args(["prepare", "--release-history", &history, "--cache"])
                .arg(&storage)
                .arg("--output")
                .arg(&prepared)
                .output()
                .unwrap(),
        );
        let other = Fixture::from_template(&fixture);
        other.git(&["commit", "--allow-empty", "-m", "new current provenance"]);
        let fresh = |output: &Path| {
            let mut command = checker_command();
            command
                .current_dir(other.path())
                .args([
                    "check-compatibility",
                    "--verbose",
                    "--release-history",
                    &history,
                    "--output",
                ])
                .arg(output);
            command
        };
        let warm = evidence.path().join("warm");
        reused(&success(
            fresh(&warm).arg("--cache").arg(&storage).output().unwrap(),
        ));
        let mut expected: Value =
            serde_json::from_slice(&fs::read(prepared.join("report.json")).unwrap()).unwrap();
        let current: Value =
            serde_json::from_slice(&fs::read(warm.join("report.json")).unwrap()).unwrap();
        assert_ne!(expected.get("head").unwrap(), current.get("head").unwrap());
        assert_eq!(current.get("head").unwrap(), &other.sha("HEAD"));
        *expected.get_mut("head").unwrap() = current.get("head").unwrap().clone();
        assert_eq!(current, expected);

        let wrong = evidence.path().join("wrong-source");
        let rejected = checker_command()
            .current_dir(other.path())
            .args(["check-compatibility", "--verbose", "--prepared"])
            .arg(prepared.join("prepared.json"))
            .arg("--cache")
            .arg(&storage)
            .arg("--output")
            .arg(&wrong)
            .output()
            .unwrap();
        assert!(!rejected.status.success());
        assert!(!wrong.join("report.json").exists());
        assert!(
            !String::from_utf8_lossy(&rejected.stderr).contains("reusing classification decisions")
        );

        other.write(
            "packages/library/src/lib.rs",
            "pub fn different_input() {}\n",
        );
        let changed = evidence.path().join("changed");
        let result = success(
            fresh(&changed)
                .arg("--cache")
                .arg(&storage)
                .output()
                .unwrap(),
        );
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("computed classification decisions")
        );
        assert!(
            !String::from_utf8_lossy(&result.stderr).contains("reusing classification decisions")
        );
        let uncached = evidence.path().join("uncached");
        success(fresh(&uncached).arg("--no-cache").output().unwrap());
        same_evidence(&changed, &uncached);
        assert_ne!(
            fs::read(warm.join("diffs/library.patch")).unwrap(),
            fs::read(changed.join("diffs/library.patch")).unwrap()
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Changes captured source and live history refs between CLI invocations"
)]
fn cached_preview_rejects_source_candidate_and_named_history_drift_before_report() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("absolute");
        reused(&success(assessment.check("warm").output().unwrap()));
        for (name, path) in [
            (
                "source",
                assessment
                    .fixture
                    .path()
                    .join("packages/library/src/lib.rs"),
            ),
            (
                "candidate",
                candidate(&assessment).join("packages/library/src/lib.rs"),
            ),
        ] {
            let original = fs::read(&path).unwrap();
            fs::write(&path, "pub fn drift() {}\n").unwrap();
            rejected_before_report(&assessment, name);
            fs::write(path, original).unwrap();
        }
        for name in ["release-history", "anticipated-parent"] {
            let original = assessment.fixture.sha(name);
            let changed = assessment.fixture.sha("HEAD");
            let reference = format!("refs/heads/{name}");
            assessment
                .fixture
                .git(&["update-ref", &reference, &changed, &original]);
            rejected_before_report(&assessment, name);
            assessment
                .fixture
                .git(&["update-ref", &reference, &original, &changed]);
        }
        reused(&success(assessment.check("restored").output().unwrap()));
        same_evidence(&assessment.path("warm"), &assessment.path("restored"));
    });
}

fn rejected_before_report(assessment: &Assessment, name: &str) {
    let result = assessment.check(name).output().unwrap();
    assert!(!result.status.success());
    assert!(!assessment.path(&format!("{name}/report.json")).exists());
    assert!(
        !assessment
            .path(&format!("{name}/compatibility.json"))
            .exists()
    );
    assert!(!assessment.path(&format!("{name}.calls")).exists());
    assert!(!String::from_utf8_lossy(&result.stderr).contains("reusing classification decisions"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Injects source drift immediately after fresh classification hashing"
)]
fn cache_hit_does_not_skip_verification_after_source_acquisition() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("default");
        let mut command = assessment.check("drift");
        configure_git_shim(&mut command, assessment.evidence.path());
        let result = command
            .env("CRP_REPORT_DRIFT_AFTER_HASH", "1")
            .env("CRP_REPORT_DRIFT_MARKER", assessment.path("mutated"))
            .output()
            .unwrap();
        reused(&result);
        assert!(!result.status.success());
        assert!(assessment.path("mutated").is_file());
        assert!(assessment.path("drift/report.json").is_file());
        assert!(!assessment.path("drift/compatibility.json").exists());
        assert!(!assessment.path("drift.calls").exists());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Runs checker failures and parent mutations after persisted cache hits"
)]
fn cache_hits_do_not_cache_checker_success_or_parent_verification() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("absolute");
        reused(&success(assessment.check("warm").output().unwrap()));
        let history = assessment.fixture.sha("release-history");
        for scenario in [
            "identity-failure",
            "canary-failure",
            "parent-comparison-failure",
            "parent-source-drift",
            "parent-head-drift",
            "parent-target-drift",
        ] {
            let result = assessment
                .check(scenario)
                .env("CRP_FIXTURE_SCENARIO", scenario)
                .env("CRP_FIXTURE_HISTORY", &history)
                .env("CRP_FIXTURE_ROOT", assessment.fixture.path())
                .output()
                .unwrap();
            reused(&result);
            assert!(!result.status.success());
            let outcome = read_outcome(&assessment.path(scenario));
            assert_eq!(outcome.get("completed").unwrap(), false);
            let calls = fs::read_to_string(assessment.path(&format!("{scenario}.calls"))).unwrap();
            assert_eq!(
                calls,
                match scenario {
                    "identity-failure" => "version\n",
                    "canary-failure" | "parent-target-drift" => "version\ncanary\n",
                    _ => "version\ncanary\ncomparison\n",
                }
            );
            assert_eq!(
                assessment
                    .fixture
                    .git(&["worktree", "list", "--porcelain"])
                    .matches("worktree ")
                    .count(),
                1
            );
            if scenario == "parent-target-drift" {
                assessment.fixture.git(&[
                    "update-ref",
                    "refs/heads/anticipated-parent",
                    &assessment.parent,
                    &history,
                ]);
            }
        }
        let result = assessment
            .check("deny")
            .arg("--deny-findings")
            .output()
            .unwrap();
        reused(&result);
        assert!(!result.status.success());
        assert_eq!(
            read_outcome(&assessment.path("deny"))
                .get("completed")
                .unwrap(),
            true
        );
        let saved = fs::read(assessment.path("deny/compatibility.json")).unwrap();
        assert!(!assessment.check("deny").output().unwrap().status.success());
        assert_eq!(
            fs::read(assessment.path("deny/compatibility.json")).unwrap(),
            saved
        );
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Retains a second preview and applies actual captured manifest edits"
)]
fn relocated_preview_and_fully_applied_source_keep_original_cache_ownership() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let assessment = Assessment::new("relative");
        let proposal = assessment.path("major.json");
        fs::write(
            &proposal,
            serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "increments": [{"name": "library", "bump": "major"}]
            }))
            .unwrap(),
        )
        .unwrap();
        let preview = |name: &str| {
            let mut command = assessment.command("preview");
            command
                .arg("--prepared")
                .arg(assessment.path("prepared/prepared.json"))
                .arg("--plan")
                .arg(&proposal)
                .arg("--output")
                .arg(assessment.path(name));
            command
        };
        success(preview("major-preview").output().unwrap());
        reused(&success(preview("relocated").output().unwrap()));
        let check = |name: &str| assessment.check_mode(name, "--plan", "relocated/plan.json");
        reused(&success(check("before").output().unwrap()));
        success(
            checker_command()
                .current_dir(assessment.fixture.path())
                .args(["apply", "--plan"])
                .arg(assessment.path("relocated/plan.json"))
                .output()
                .unwrap(),
        );
        assert!(
            assessment
                .fixture
                .read("packages/library/Cargo.toml")
                .contains("2.0.0")
        );
        reused(&success(check("after").output().unwrap()));
        same_evidence(&assessment.path("before"), &assessment.path("after"));
        assert_eq!(
            serde_json::from_slice::<Value>(
                &fs::read(assessment.path("after/report.json")).unwrap()
            )
            .unwrap()
            .pointer("/packages/0/declared_version")
            .unwrap(),
            "2.0.0"
        );
    });
}
