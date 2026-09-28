//! Publication command integration over hermetic Git and process fixtures, without live services.

#![cfg_attr(coverage_nightly, coverage(off))]

use std::env::consts::EXE_SUFFIX;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};
use crp_publication::publication::credentials::CredentialSession;
use crp_publication::publication::github::PlatformBatch;
use crp_publication::publication::identity::TrustedPublisher;
use crp_publication::publication::manifest::PublicationManifest;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::fixture::{Fixture, write_binary_package, write_package};
use crate::harness::resolved_plan;

#[test]
#[cfg_attr(
    miri,
    ignore = "Loads publication configuration and real Git/Cargo source"
)]
fn publication_configuration_is_explicit_and_does_not_replace_version_checks() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    // Match the fixture's release line; no binary targets are needed for this library-only case.
    let configuration =
        "schema-version = 1\nrepository = 'example/libs'\nrelease-branch = 'main'\ntargets = []";
    fixture.write(".cargo/release_plan.toml", configuration);
    fixture.commit("configure publication");
    let base = fixture.sha("HEAD");
    let input = |configured: bool| RunInput::Check {
        merge_target: None,
        base: Some(base.clone()),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        config: configured.then(|| PathBuf::from(".cargo/release_plan.toml")),
        verbose: false,
    };
    assert!(matches!(
        run(&input(true)).unwrap(),
        RunOutcome::Check { passed: true, .. }
    ));
    fixture.write(".cargo/release_plan.toml", "not valid TOML");
    run(&input(true)).unwrap_err();
    assert!(matches!(
        run(&input(false)).unwrap(),
        RunOutcome::Check { passed: true, .. }
    ));
    fixture.write(".cargo/release_plan.toml", configuration);
    fixture.write("packages/demo/src/lib.rs", "pub fn new_operation() {}\n");
    assert!(matches!(
        run(&input(true)).unwrap(),
        RunOutcome::Check { passed: false, .. }
    ));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Uses Git and Cargo metadata for offline publication checks"
)]
fn offline_check_accepts_a_feature_gated_binary_with_restricted_targets() {
    let fixture = Fixture::new("");
    // Representative compatible versions keep the case focused on metadata and target restrictions.
    write_package(
        &fixture,
        "optional-core",
        "0.1.0",
        "[features]\nenhanced = []\n",
    );
    write_binary_package(
        &fixture,
        "tool",
        "0.1.0",
        r#"repository = "https://github.com/example/tools"
[[bin]]
name = "tool"
path = "src/main.rs"
required-features = ["optional-core"]
[dependencies]
optional-core = { path = "../optional-core", version = "0.1.0", optional = true }
[features]
default = ["optional-core/enhanced"]
[package.metadata.release-plan]
release-targets = ["x86_64-pc-windows-msvc"]
[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir = "{ bin }{ binary-ext }"
pkg-fmt = "zip"
"#,
    );
    // The branch matches the fixture; the repository deliberately offers an unselected target.
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/tools'\nrelease-branch = 'main'\n\
         targets = ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']",
    );
    // Offline checking needs the lockfile, not a build or a second feature-selection oracle.
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(fixture.path().join("Cargo.lock").is_file());
    fixture.commit("configured binary source");
    assert!(matches!(
        run(&RunInput::Check {
            merge_target: None,
            base: Some(fixture.sha("HEAD")),
            manifest_path: fixture.manifest(),
            format: CheckFormat::Text,
            verify_packaging: false,
            config: Some(PathBuf::from(".cargo/release_plan.toml")),
            verbose: true,
        })
        .unwrap(),
        RunOutcome::Check { passed: true, .. }
    ));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Discovers and previews real nonpublishable Cargo/Git members"
)]
fn publication_preflight_does_not_query_or_change_nonpublishable_members() {
    let fixture = Fixture::new("");
    write_package(&fixture, "helper", "1.0.0", "publish = false\n");
    fixture.commit("local-only workspace");
    fixture.write("proposal.json", r#"{"schema_version":5,"increments":[]}"#);
    let plan = resolved_plan(&fixture, &fixture.path().join("proposal.json"));
    let manifest = fs::read(fixture.manifest()).unwrap();
    let lockfile = fixture.read("Cargo.lock");
    let status = fixture.git(&["status", "--porcelain"]);
    for plan in [None, Some(plan)] {
        let RunOutcome::Check {
            passed,
            message,
            warnings,
        } = run(&RunInput::CheckPublished {
            manifest_path: fixture.manifest(),
            plan,
            verbose: true,
        })
        .unwrap()
        else {
            panic!()
        };
        assert!(passed);
        assert!(!message.is_empty());
        assert!(warnings.is_empty());
    }
    assert_eq!(fs::read(fixture.manifest()).unwrap(), manifest);
    assert_eq!(fixture.read("Cargo.lock"), lockfile);
    assert_eq!(fixture.git(&["status", "--porcelain"]), status);
}

#[test]
#[cfg_attr(miri, ignore = "Discovers an owned Cargo workspace")]
fn publication_preflight_accepts_a_workspace_with_no_publishable_targets() {
    let fixture = Fixture::new("");
    write_package(&fixture, "private-helper", "1.0.0", "publish = false\n");
    fixture.commit("private workspace");
    let RunOutcome::Check {
        passed,
        message,
        warnings,
    } = run(&RunInput::CheckPublished {
        manifest_path: fixture.manifest(),
        plan: None,
        verbose: true,
    })
    .unwrap()
    else {
        panic!("publication preflight must return a check verdict");
    };
    assert!(passed);
    assert!(warnings.is_empty());
    assert_eq!(
        message,
        "Every selected publishable package is established on crates.io."
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Attempts Cargo workspace acquisition from an absent manifest"
)]
fn publication_preflight_propagates_workspace_acquisition_failure() {
    let directory = TempDir::new().unwrap();
    _ = run(&RunInput::CheckPublished {
        manifest_path: directory.path().join("absent").join("Cargo.toml"),
        plan: None,
        verbose: false,
    })
    .unwrap_err();
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Builds and archives an immutable native source worktree"
)]
// Mirror publication::config::NativeTarget: this exercises supported native hosts, not cross-builds.
#[cfg(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(
        all(target_os = "windows", target_env = "msvc"),
        all(target_os = "linux", target_env = "gnu"),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
fn unified_binary_command_stages_a_frozen_batch_without_github() {
    // As in native_binaries::SMOKE_WATCHDOG, allow orders of magnitude more than ordinary
    // seconds-long toolchain/archive runs. This watchdog is not a test failure assertion.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        // Ordinary successor identity; manifest, batch, tag and archive names must agree.
        const VERSION: &str = "1.0.1";
        let fixture = publication_source();
        write_binary_package(
            &fixture,
            "library",
            VERSION,
            r#"
repository = "https://github.com/example/publication-fixture"
[[bin]]
name = "different-executable"
path = "src/main.rs"
[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir = "{ bin }{ binary-ext }"
pkg-fmt = "zip"
"#,
        );
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        fs::copy(
            repository.join("rust-toolchain.toml"),
            fixture.path().join("rust-toolchain.toml"),
        )
        .unwrap();
        let compiler = Command::new("rustc")
            .args(["--version", "--verbose"])
            .output()
            .unwrap();
        assert!(compiler.status.success());
        let compiler = String::from_utf8(compiler.stdout).unwrap();
        let target = compiler
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        fixture.write(".cargo/release_plan.toml", &format!(
            "schema-version=1\nrepository='example/publication-fixture'\nrelease-branch='stable'\ntargets=['{target}']\n"
        ));
        let cargo = Command::new("cargo")
            .args(["generate-lockfile", "--offline"])
            .current_dir(fixture.path())
            .output()
            .unwrap();
        assert!(
            cargo.status.success(),
            "{}",
            String::from_utf8_lossy(&cargo.stderr)
        );
        fixture.commit("binary publication source");
        fixture.git(&["branch", "--force", "stable", "HEAD"]);
        let output = TempDir::new().unwrap();
        let publication = output.path().join("publication.json");
        run(&preparation(&fixture, publication.clone())).unwrap();
        let intent = fs::read(&publication).unwrap();
        let parsed: Value = serde_json::from_slice(&intent).unwrap();
        let batch = output.path().join("batch.json");
        let input_batch: PlatformBatch = serde_json::from_value(json!({
            "schema_version":1,"publication_id":parsed.get("id").unwrap(),
            "repository":"example/publication-fixture","target":target,"batch_id":"",
            "binaries":[{
                "name":"library","bin":"different-executable","version":VERSION,
                "tag":format!("library-v{VERSION}"),"source_sha":fixture.sha("HEAD")
            }]
        }))
        .unwrap();
        let input_batch = input_batch.seal().unwrap();
        fs::write(&batch, serde_json::to_vec(&input_batch).unwrap()).unwrap();
        let outcome = output.path().join("outcome.json");
        let artifacts = output.path().join("artifacts");
        let result = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args(["publish", "binaries", "--publication"])
            .arg(&publication)
            .arg("--batch")
            .arg(&batch)
            .arg("--manifest-path")
            .arg(fixture.manifest())
            .arg("--output")
            .arg(&outcome)
            .arg("--artifacts")
            .arg(&artifacts)
            .arg("--no-upload")
            .env("CARGO_TARGET_DIR", output.path().join("cache"))
            .env_remove("GITHUB_STEP_SUMMARY")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let outcome: Value = serde_json::from_slice(&fs::read(outcome).unwrap()).unwrap();
        assert_eq!(outcome.get("complete").unwrap(), false);
        assert_eq!(outcome.get("batch_id").unwrap(), &input_batch.batch_id);
        assert_eq!(outcome.pointer("/items/0/status").unwrap(), "staged-only");
        let base = format!("library-v{VERSION}-{target}");
        let staging = artifacts.join(&base);
        assert!(
            staging
                .join(format!("different-executable{EXE_SUFFIX}"))
                .is_file()
        );
        assert!(staging.join(format!("{base}.zip")).is_file());
        assert!(staging.join(format!("{base}.sha256")).is_file());
        assert_eq!(fs::read(publication).unwrap(), intent);
        assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves configured release history with Git and Cargo"
)]
fn release_context_resolves_history_without_requiring_clean_source() {
    let fixture = publication_source();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn pending_change() {}\n",
    );
    for explicit in [None, Some(base.clone())] {
        let outcome = run(&RunInput::ReleaseContext {
            merge_target: None,
            manifest_path: fixture.manifest(),
            config: None,
            base: explicit,
            verbose: true,
        })
        .unwrap();
        let RunOutcome::ArtifactQuery { message } = outcome else {
            panic!()
        };
        let context: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(context.get("schema_version").unwrap(), 2);
        assert_eq!(context.get("release_history").unwrap(), &base);
        assert!(context.get("merge_target").unwrap().is_null());
        assert_eq!(
            context.get("repository").unwrap(),
            "example/publication-fixture"
        );
        assert_eq!(context.get("workspace_manifest").unwrap(), "Cargo.toml");
        assert!(
            context
                .get("concurrency_group")
                .unwrap()
                .as_str()
                .unwrap()
                .starts_with("cargo-release-plan-")
        );
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves release and parent commits in a local Git fixture"
)]
fn release_context_keeps_actual_history_distinct_from_the_parent_target() {
    let fixture = publication_source();
    let history = fixture.sha("HEAD");
    fixture.write("packages/library/src/lib.rs", "pub fn parent_change() {}\n");
    fixture.commit("parent final content");
    let parent = fixture.sha("HEAD");
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn uncommitted_child() {}\n",
    );
    let mut concurrency = None;
    for (target, expected) in [
        (None, None),
        (Some(history.clone()), None),
        (Some(parent.clone()), Some(parent.as_str())),
    ] {
        let RunOutcome::ArtifactQuery { message } = run(&RunInput::ReleaseContext {
            manifest_path: fixture.manifest(),
            config: None,
            base: Some(history.clone()),
            merge_target: target,
            verbose: false,
        })
        .unwrap() else {
            panic!()
        };
        let context: Value = serde_json::from_str(&message).unwrap();
        assert_eq!(context.get("release_history").unwrap(), &history);
        assert_eq!(context.get("merge_target").unwrap().as_str(), expected);
        let group = context.get("concurrency_group").unwrap();
        if let Some(previous) = &concurrency {
            assert_eq!(group, previous);
        } else {
            concurrency = Some(group.clone());
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn prepares_frozen_requests_and_reuses_identical_output_without_changing_source() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    let manifest: Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(
        manifest.pointer("/publication/source").unwrap(),
        &fixture.sha("HEAD")
    );
    assert_eq!(
        manifest.pointer("/publication/workspace_manifest").unwrap(),
        "Cargo.toml"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/name").unwrap(),
        "library"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/version").unwrap(),
        "1.0.0"
    );
    assert_eq!(
        manifest
            .pointer("/publication/configuration/release-branch")
            .unwrap(),
        "stable"
    );
    assert_eq!(
        manifest.pointer("/publication/packages/0/binary").unwrap(),
        &Value::Null
    );
    assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    run(&input).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn rejects_dirty_tracked_publication_inputs_without_writing_an_artifact() {
    // Real-Git untracked rejection is covered by crp_publication's candidate boundary test
    // rejects_staged_and_untracked_inputs; this case verifies preparation leaves no artifact.
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
    run(&input).unwrap_err();
    assert!(!path.exists());
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn branch_movement_does_not_change_intent_for_the_same_source() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    let source = fixture.sha("HEAD");
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    fixture.write("notes.txt", "unrelated release-branch movement\n");
    fixture.commit("advance release line");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    fixture.git(&["checkout", "--detach", &source]);
    run(&input).unwrap();
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against a temporary workspace")]
fn rejects_different_existing_intent_without_overwriting_it() {
    let fixture = publication_source();
    let output = TempDir::new().unwrap();
    let path = output.path().join("publication.json");
    let input = preparation(&fixture, path.clone());
    run(&input).unwrap();
    let original = fs::read(&path).unwrap();
    fixture.write(
        "notes.txt",
        "another commit with identical package versions\n",
    );
    fixture.commit("advance release line");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    run(&preparation(&fixture, path.clone())).unwrap_err();
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
#[cfg_attr(miri, ignore = "Executes Cargo and Git and persists phase outcomes")]
fn empty_publication_phases_need_no_identity_and_never_overwrite_intent() {
    let fixture = publication_source();
    write_package(&fixture, "library", "1.0.0", "publish = false\n");
    fixture.commit("private workspace");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    let output = TempDir::new().unwrap();
    let publication = output.path().join("publication.json");
    run(&preparation(&fixture, publication.clone())).unwrap();
    let intent = fs::read(&publication).unwrap();
    for dry_run in [false, true] {
        for phase in ["registry", "github"] {
            let outcome = output.path().join(format!("{phase}-{dry_run}.json"));
            let input = if phase == "github" {
                RunInput::PublishGithub {
                    publication: publication.clone(),
                    manifest_path: fixture.manifest(),
                    output: outcome.clone(),
                    batches: output.path().join(format!("batches-{dry_run}")),
                    dry_run,
                    verbose: false,
                }
            } else {
                RunInput::PublishRegistry {
                    publication: publication.clone(),
                    manifest_path: fixture.manifest(),
                    output: outcome.clone(),
                    dry_run,
                    verbose: false,
                }
            };
            assert!(matches!(
                run(&input).unwrap(),
                RunOutcome::Publication { passed: true, .. }
            ));
            let report: Value = serde_json::from_slice(&fs::read(&outcome).unwrap()).unwrap();
            assert_eq!(report.get("complete").unwrap(), !dry_run);
            assert_eq!(report.get("dry_run").unwrap(), dry_run);
            assert!(
                report
                    .get("packages")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            let manifest: Value = serde_json::from_slice(&intent).unwrap();
            assert_eq!(report.get("publication_id"), manifest.get("id"));
            run(&input).unwrap_err();
        }
    }
    run(&RunInput::PublishRegistry {
        publication: publication.clone(),
        manifest_path: fixture.manifest(),
        output: publication.clone(),
        dry_run: false,
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fs::read(&publication).unwrap(), intent);

    fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
    for github in [false, true] {
        let outcome = output.path().join(format!("invalid-source-{github}.json"));
        let input = if github {
            RunInput::PublishGithub {
                publication: publication.clone(),
                manifest_path: fixture.manifest(),
                output: outcome.clone(),
                batches: output.path().join("invalid-source-batches"),
                dry_run: false,
                verbose: false,
            }
        } else {
            RunInput::PublishRegistry {
                publication: publication.clone(),
                manifest_path: fixture.manifest(),
                output: outcome.clone(),
                dry_run: false,
                verbose: false,
            }
        };
        assert!(matches!(
            run(&input).unwrap(),
            RunOutcome::Publication { passed: false, .. }
        ));
        let report: Value = serde_json::from_slice(&fs::read(outcome).unwrap()).unwrap();
        assert_eq!(report.get("complete").unwrap(), false);
        assert!(!report.get("errors").unwrap().as_array().unwrap().is_empty());
    }
}

#[test]
#[cfg_attr(miri, ignore = "Executes the reporter with isolated workflow identity")]
fn reporter_retains_job_failure_when_the_outcome_directory_is_a_file() {
    testing::with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let publication = directory.path().join("publication.json");
        // Reporting consumes an artifact, not a source checkout. An opaque source identity
        // and empty request set keep this scenario focused on failed artifact acquisition.
        PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/reporting","release-branch":"main","targets":[]},
            "packages":[]
        })).unwrap()).unwrap().write(&publication).unwrap();
        let outcomes = directory.path().join("outcomes-as-file");
        let downloaded = b"downloaded artifact occupies the expected directory path";
        fs::write(&outcomes, downloaded).unwrap();
        let jobs = directory.path().join("jobs.json");
        fs::write(
            &jobs,
            br#"{"prepare":"success","registry":"failure","github":"skipped","binaries":"skipped"}"#,
        )
        .unwrap();
        let output = directory.path().join("report.md");
        let result = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args([
                "publish",
                "report",
                "--repository",
                "example/reporting",
                "--publication",
            ])
            .arg(&publication)
            .arg("--outcomes")
            .arg(&outcomes)
            .arg("--jobs")
            .arg(&jobs)
            .arg("--output")
            .arg(&output)
            .arg("--no-issue")
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "2")
            .output()
            .unwrap();
        assert!(!result.status.success());
        let body = fs::read_to_string(output).unwrap();
        assert!(body.contains("Release incomplete"));
        assert!(body.contains(&outcomes.display().to_string()));
        assert!(
            body.lines()
                .any(|line| line.contains("registry job") && line.contains("Failure"))
        );
        assert!(body.contains("https://github.com/example/reporting/actions/runs/123"));
        assert!(String::from_utf8_lossy(&result.stderr).contains("outcomes-as-file"));
        assert_eq!(fs::read(outcomes).unwrap(), downloaded);
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes the reporter with isolated workflow identity")]
fn reporter_preserves_job_failures_when_publication_artifacts_are_missing_or_invalid() {
    let fixture = publication_source();
    let directory = TempDir::new().unwrap();
    let publication = directory.path().join("publication.json");
    run(&preparation(&fixture, publication.clone())).unwrap();
    let jobs = directory.path().join("jobs.json");
    fs::write(
        &jobs,
        br#"{"prepare":"success","registry":"failure","github":"skipped","binaries":"skipped"}"#,
    )
    .unwrap();
    let missing = directory.path().join("missing-outcomes");
    let invalid = directory.path().join("invalid-outcomes");
    fs::create_dir_all(&invalid).unwrap();
    fs::write(invalid.join("outcome.json"), b"{").unwrap();
    for (outcomes, has_intent) in [(missing, false), (invalid, true)] {
        let output = outcomes.with_extension("md");
        let result = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args([
                "publish",
                "report",
                "--repository",
                "example/publication-fixture",
                "--outcomes",
            ])
            .arg(&outcomes)
            .arg("--publication")
            .arg(if has_intent {
                publication.clone()
            } else {
                directory.path().join("missing-publication.json")
            })
            .arg("--jobs")
            .arg(&jobs)
            .arg("--output")
            .arg(&output)
            .arg("--no-issue")
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "2")
            .output()
            .unwrap();
        assert!(!result.status.success());
        let body = fs::read_to_string(output).unwrap();
        assert!(body.contains("https://github.com/example/publication-fixture/actions/runs/123"));
        assert!(body.contains("unavailable"));
        assert!(body.contains("original failed workflow"));
        assert!(body.contains("registry job"));
        assert!(body.contains("github job"));
        if has_intent {
            let diagnostic = String::from_utf8_lossy(&result.stderr);
            assert!(diagnostic.contains("outcome.json"));
            assert!(diagnostic.contains("cannot read publication evidence"));
        }
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Exercises the report executable with real persisted outcomes"
)]
fn reporter_completes_valid_delivery_and_rejects_ambiguous_receipts_and_destinations() {
    let fixture = publication_source();
    let directory = TempDir::new().unwrap();
    let publication = directory.path().join("publication.json");
    run(&preparation(&fixture, publication.clone())).unwrap();
    let intent: Value = serde_json::from_slice(&fs::read(&publication).unwrap()).unwrap();
    let outcomes = directory.path().join("outcomes");
    let captured = intent.get("publication").unwrap();
    let packages = captured.get("packages").unwrap().as_array().unwrap();
    let registry_packages: Vec<_> = packages
        .iter()
        .map(|package| {
            json!({
                "name":package.get("name").unwrap(),
                "version":package.get("version").unwrap(),
                "state":"already_present"
            })
        })
        .collect();
    let github_packages: Vec<_> = packages
        .iter()
        .map(|package| {
            let name = package.get("name").unwrap().as_str().unwrap();
            let version = package.get("version").unwrap().as_str().unwrap();
            json!({
                "name":name,"version":version,"tag":format!("{name}-v{version}"),
                "state":"complete","source":captured.get("source").unwrap(),
                "recovery_source":null,"observed_version":null
            })
        })
        .collect();
    let registry = json!({
        "schema_version":1,"publication_id":intent.get("id").unwrap(),
        "phase":"registry","complete":true,"dry_run":false,
        "packages":registry_packages,"errors":[],"notes":[],
        "github":{"run_id":123,"run_attempt":1}
    });
    let github = json!({
        "schema_version":1,"publication_id":intent.get("id").unwrap(),
        "phase":"github","complete":true,"dry_run":false,
        "packages":github_packages,"batches":[],"planned_targets":[],"errors":[],
        "github":{"run_id":123,"run_attempt":1}
    });
    for (phase, outcome) in [("registry", &registry), ("github", &github)] {
        fs::create_dir_all(outcomes.join(phase)).unwrap();
        fs::write(
            outcomes.join(phase).join("outcome.json"),
            serde_json::to_vec(outcome).unwrap(),
        )
        .unwrap();
    }
    let jobs = directory.path().join("jobs.json");
    fs::write(
        &jobs,
        br#"{"prepare":"success","registry":"success","github":"success","binaries":"skipped"}"#,
    )
    .unwrap();
    let command = |output: &Path, repository: &str| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
        command
            .args([
                "publish",
                "report",
                "--repository",
                repository,
                "--outcomes",
            ])
            .arg(&outcomes)
            .arg("--publication")
            .arg(&publication)
            .arg("--jobs")
            .arg(&jobs)
            .arg("--output")
            .arg(output)
            .arg("--no-issue")
            .env("GITHUB_ACTIONS", "true")
            .env("GITHUB_RUN_ID", "123")
            .env("GITHUB_RUN_ATTEMPT", "1");
        command
    };
    let output = directory.path().join("complete.md");
    assert!(
        command(&output, "example/publication-fixture")
            .output()
            .unwrap()
            .status
            .success()
    );
    let complete = fs::read(&output).unwrap();
    assert!(String::from_utf8_lossy(&complete).contains("Release complete"));
    assert!(
        !command(&output, "example/publication-fixture")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert_eq!(fs::read(&output).unwrap(), complete);
    let wrong = directory.path().join("wrong-repository.md");
    assert!(
        !command(&wrong, "example/other")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!wrong.exists());

    fs::create_dir_all(outcomes.join("duplicate")).unwrap();
    let current_schema = registry.get("schema_version").unwrap().as_u64().unwrap();
    // Duplicate valid receipts and unsupported schemas are distinct rejection paths.
    for (label, schema) in [
        ("duplicate", current_schema),
        ("unsupported", current_schema + 1),
    ] {
        let mut duplicate = registry.clone();
        *duplicate.get_mut("schema_version").unwrap() = json!(schema);
        let receipt = if label == "duplicate" {
            outcomes.join("duplicate").join("outcome.json")
        } else {
            outcomes.join("registry").join("outcome.json")
        };
        fs::write(&receipt, serde_json::to_vec(&duplicate).unwrap()).unwrap();
        let output = directory.path().join(format!("{label}.md"));
        let result = command(&output, "example/publication-fixture")
            .output()
            .unwrap();
        assert!(!result.status.success());
        let body = fs::read_to_string(output).unwrap();
        assert!(body.contains("Release incomplete"));
        if label == "duplicate" {
            assert!(body.contains("Duplicate registry outcomes"));
            assert!(body.contains(&receipt.display().to_string()));
            assert!(
                body.contains(
                    &outcomes
                        .join("registry")
                        .join("outcome.json")
                        .display()
                        .to_string()
                )
            );
        } else {
            assert!(String::from_utf8_lossy(&result.stderr).contains("registry schema"));
        }
        if label == "duplicate" {
            fs::remove_file(receipt).unwrap();
        }
    }
    fs::write(
        outcomes.join("registry/outcome.json"),
        serde_json::to_vec(&registry).unwrap(),
    )
    .unwrap();
    fs::write(&publication, b"{").unwrap();
    let malformed = directory.path().join("malformed-manifest.md");
    assert!(
        !command(&malformed, "example/publication-fixture")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        fs::read_to_string(malformed)
            .unwrap()
            .contains("Release incomplete")
    );
    let unhosted = directory.path().join("unhosted.md");
    assert!(
        !command(&unhosted, "example/publication-fixture")
            .env_remove("GITHUB_ACTIONS")
            .env_remove("GITHUB_RUN_ID")
            .env_remove("GITHUB_RUN_ATTEMPT")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!unhosted.exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Exercises private credential protocol routing in an isolated process"
)]
fn credential_provider_entry_requires_context_and_rejects_unrequested_uploads() {
    // This finite protocol normally finishes in seconds; the large budget is only a last-chance
    // guard for loaded/instrumented hosts, never an expected protocol-failure timeout.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let directory = TempDir::new().unwrap();
        let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main","targets":[]},
            "packages":[]
        })).unwrap()).unwrap();
        let session = CredentialSession::new(
            serde_json::from_value(json!({
                "request_url":"http://127.0.0.1:0/unused",
                "request_token":"identity-credential-canary"
            }))
            .unwrap(),
            publication,
            directory.path().join("unused-source/Cargo.toml"),
            directory.path().join("target"),
            TrustedPublisher::with_endpoint(
                "http://127.0.0.1:0/unused",
                crp_publication::PublicationOutput::new(
                    "1.2.3",
                    false,
                    std::sync::Arc::new(crp_diag::Discard),
                ),
            )
            .unwrap(),
        )
        .unwrap();
        let mut cargo = Command::new("cargo");
        session
            .configure(&mut cargo, Path::new("provider"))
            .unwrap();
        let context = cargo
            .get_envs()
            .find(|(name, _)| *name == "CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT")
            .unwrap()
            .1
            .unwrap();
        let executable = env!("CARGO_BIN_EXE_cargo-release-plan");
        let missing = Command::new(executable)
            .arg("--cargo-plugin")
            .env_remove("CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT")
            .output()
            .unwrap();
        assert!(!missing.status.success());
        assert!(missing.stdout.is_empty());
        let mut child = Command::new(executable)
            .arg("--cargo-plugin")
            .env("CARGO_RELEASE_PLAN_CREDENTIAL_CONTEXT", context)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Independently check Cargo's credential-provider wire protocol rather than importing
        // the producer's revision constant. Keep this request and the advertised hello paired.
        // Ref: https://doc.rust-lang.org/cargo/reference/credential-provider-protocol.html
        let request = json!({
            "v":1,"kind":"get","operation":"publish","name":"unrequested","vers":"1.0.0",
            "cksum":"b".repeat(64),"registry":{"index-url":"sparse+https://index.crates.io/"}
        });
        child
            .stdin
            .take()
            .unwrap()
            .write_all(request.to_string().as_bytes())
            .unwrap();
        let rejected = child.wait_with_output().unwrap();
        assert!(!rejected.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&rejected.stdout).unwrap(),
            json!({"v":[1]})
        );
        assert!(!String::from_utf8_lossy(&rejected.stderr).contains("identity-credential-canary"));
        session.finish().unwrap();
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Exercises identity environment validation in isolated processes"
)]
fn identity_probe_rejects_missing_or_empty_job_identity_without_network_access() {
    for scenario in [
        "missing-url",
        "empty-url",
        "missing-token",
        "empty-token",
        "invalid-url",
    ] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
        command
            .args(["check-publishing-identity", "--verbose"])
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_URL")
            .env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
        // Every case has exactly one invalid input. Validation must fail before transport.
        command
            .env("ACTIONS_ID_TOKEN_REQUEST_URL", "http://127.0.0.1:0/unused")
            .env(
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                "identity-credential-canary",
            );
        let expected = match scenario {
            "missing-url" => {
                command.env_remove("ACTIONS_ID_TOKEN_REQUEST_URL");
                "ACTIONS_ID_TOKEN_REQUEST_URL"
            }
            "empty-url" => {
                command.env("ACTIONS_ID_TOKEN_REQUEST_URL", "");
                "ACTIONS_ID_TOKEN_REQUEST_URL"
            }
            "missing-token" => {
                command.env_remove("ACTIONS_ID_TOKEN_REQUEST_TOKEN");
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN"
            }
            "empty-token" => {
                command.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
                "ACTIONS_ID_TOKEN_REQUEST_TOKEN"
            }
            "invalid-url" => {
                command.env("ACTIONS_ID_TOKEN_REQUEST_URL", "not an identity URL");
                "GitHub OIDC endpoint"
            }
            _ => unreachable!(),
        };
        let result = command.output().unwrap();
        assert!(!result.status.success());
        let diagnostic = String::from_utf8_lossy(&result.stderr);
        assert!(diagnostic.contains(expected));
        assert!(!diagnostic.contains("identity-credential-canary"));
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Validates publication source before any registry query"
)]
fn nonempty_registry_intent_retains_unknown_package_states_when_source_is_invalid() {
    let fixture = publication_source();
    let directory = TempDir::new().unwrap();
    let publication = directory.path().join("publication.json");
    run(&preparation(&fixture, publication.clone())).unwrap();
    fixture.write("packages/library/src/lib.rs", "pub fn changed() {}\n");
    let output = directory.path().join("registry.json");
    assert!(matches!(
        run(&RunInput::PublishRegistry {
            publication,
            manifest_path: fixture.manifest(),
            output: output.clone(),
            dry_run: false,
            verbose: true,
        })
        .unwrap(),
        RunOutcome::Publication { passed: false, .. }
    ));
    let result: Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(result.pointer("/packages/0/state").unwrap(), "unknown");
    assert_eq!(result.get("complete").unwrap(), false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Verifies that invalid publication sources create no artifact"
)]
fn publication_preparation_rejects_symbolic_or_abbreviated_source_identities() {
    let directory = TempDir::new().unwrap();
    for source in ["HEAD", "abc123", ""] {
        let output = directory.path().join("publication.json");
        let error = run(&RunInput::PreparePublish {
            manifest_path: directory.path().join("unused/Cargo.toml"),
            config: None,
            source: source.to_owned(),
            output: output.clone(),
            verbose: true,
        })
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("source must be a full immutable commit ID")
        );
        assert!(!output.exists());
    }
}

fn preparation(fixture: &Fixture, output: PathBuf) -> RunInput {
    RunInput::PreparePublish {
        manifest_path: fixture.manifest(),
        config: None,
        source: fixture.sha("HEAD"),
        output,
        verbose: true,
    }
}

fn publication_source() -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "1.0.0", "");
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/publication-fixture'\n\
         release-branch = 'stable'\ntargets = []\n",
    );
    let output = Command::new("cargo")
        .args(["generate-lockfile", "--offline"])
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    fixture.commit("publication source");
    fixture.git(&["branch", "stable", "HEAD"]);
    // Git's local URL rewrite exercises production fetch arguments without a network service.
    fixture.git(&[
        "config",
        &format!("url.{}.insteadOf", fixture.path().display()),
        "https://github.com/example/publication-fixture.git",
    ]);
    fixture
}
