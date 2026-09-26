//! Immutable publication preparation using a local Git transport, never a live forge.

use std::env::consts::EXE_SUFFIX;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use cargo_release_plan::{RunInput, RunOutcome, run};
use crp_publication::publication::credentials::CredentialSession;
use crp_publication::publication::github::PlatformBatch;
use crp_publication::publication::identity::TrustedPublisher;
use crp_publication::publication::manifest::PublicationManifest;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::fixture::{Fixture, write_binary_package, write_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "Builds and archives an immutable native source worktree"
)]
#[cfg(all(
    any(target_arch = "x86_64", target_arch = "aarch64"),
    any(
        all(target_os = "windows", target_env = "msvc"),
        all(target_os = "linux", target_env = "gnu"),
        all(target_os = "macos", target_arch = "aarch64")
    )
))]
fn unified_binary_command_stages_a_frozen_batch_without_github() {
    // Native toolchain and Cargo startup belong under a last-chance integration watchdog.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = publication_source();
        write_binary_package(
            &fixture,
            "library",
            "1.0.1",
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
                "name":"library","bin":"different-executable","version":"1.0.1",
                "tag":"library-v1.0.1","source_sha":fixture.sha("HEAD")
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
        let staging = artifacts.join(format!("library-v1.0.1-{target}"));
        assert!(
            staging
                .join(format!("different-executable{EXE_SUFFIX}"))
                .is_file()
        );
        assert!(
            staging
                .join(format!("library-v1.0.1-{target}.zip"))
                .is_file()
        );
        assert!(
            staging
                .join(format!("library-v1.0.1-{target}.sha256"))
                .is_file()
        );
        assert_eq!(fs::read(publication).unwrap(), intent);
        assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    });
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Resolves configured release history with Git and Cargo"
)]
fn release_context_resolves_baseline_without_requiring_clean_source() {
    let fixture = publication_source();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/library/src/lib.rs",
        "pub fn pending_change() {}\n",
    );
    for explicit in [None, Some(base.clone())] {
        let outcome = run(&RunInput::ReleaseContext {
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
        assert_eq!(context.get("release_base").unwrap(), &base);
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
fn rejects_dirty_or_untracked_publication_inputs_without_writing_an_artifact() {
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
fn reporter_preserves_job_failures_when_publication_artifacts_are_missing_or_invalid() {
    let directory = TempDir::new().unwrap();
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
    for outcomes in [missing, invalid] {
        let output = outcomes.with_extension("md");
        let result = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args([
                "publish",
                "report",
                "--repository",
                "example/tool",
                "--outcomes",
            ])
            .arg(outcomes)
            .arg("--publication")
            .arg(directory.path().join("missing-publication.json"))
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
        assert!(body.contains("https://github.com/example/tool/actions/runs/123"));
        assert!(body.contains("unavailable"));
        assert!(body.contains("original failed workflow"));
        assert!(body.contains("registry job"));
        assert!(body.contains("github job"));
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
    for phase in ["registry", "github"] {
        fs::create_dir_all(outcomes.join(phase)).unwrap();
        fs::write(
            outcomes.join(phase).join("outcome.json"),
            serde_json::to_vec(&json!({
                "schema_version":1,"publication_id":intent.get("id").unwrap(),
                "phase":phase,"complete":true,"github":{"run_id":123,"run_attempt":1}
            }))
            .unwrap(),
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
    for schema in [1, 2] {
        fs::write(
            outcomes.join("duplicate/outcome.json"),
            serde_json::to_vec(&json!({
                "schema_version":schema,"publication_id":intent.get("id").unwrap(),
                "phase":"registry","complete":true,"github":{"run_id":123,"run_attempt":1}
            }))
            .unwrap(),
        )
        .unwrap();
        let output = directory.path().join(format!("invalid-{schema}.md"));
        assert!(
            !command(&output, "example/publication-fixture")
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            fs::read_to_string(output)
                .unwrap()
                .contains("Release incomplete")
        );
    }
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
            TrustedPublisher::with_endpoint("http://127.0.0.1:0/unused").unwrap(),
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
        if scenario == "empty-url" {
            command.env("ACTIONS_ID_TOKEN_REQUEST_URL", "");
        } else if scenario != "missing-url" {
            command.env("ACTIONS_ID_TOKEN_REQUEST_URL", "not an identity URL");
            if scenario == "empty-token" {
                command.env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "");
            } else if scenario == "invalid-url" {
                command.env(
                    "ACTIONS_ID_TOKEN_REQUEST_TOKEN",
                    "identity-credential-canary",
                );
            }
        }
        assert!(!command.output().unwrap().status.success());
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
        run(&RunInput::PreparePublish {
            manifest_path: directory.path().join("unused/Cargo.toml"),
            config: None,
            source: source.to_owned(),
            output: output.clone(),
            verbose: true,
        })
        .unwrap_err();
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
