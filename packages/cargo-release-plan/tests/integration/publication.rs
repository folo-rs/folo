//! Immutable publication preparation using a local Git transport, never a live forge.

use std::env::consts::EXE_SUFFIX;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cargo_release_plan::{RunInput, RunOutcome, run};
use crp_impl::publication::github::PlatformBatch;
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
            verbose: false,
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
fn empty_registry_work_needs_no_identity_and_never_overwrites_intent() {
    let fixture = publication_source();
    write_package(&fixture, "library", "1.0.0", "publish = false\n");
    fixture.commit("private workspace");
    fixture.git(&["branch", "--force", "stable", "HEAD"]);
    let output = TempDir::new().unwrap();
    let publication = output.path().join("publication.json");
    run(&preparation(&fixture, publication.clone())).unwrap();
    let intent = fs::read(&publication).unwrap();
    for dry_run in [false, true] {
        let outcome = output.path().join(format!("registry-{dry_run}.json"));
        let input = RunInput::PublishRegistry {
            publication: publication.clone(),
            manifest_path: fixture.manifest(),
            output: outcome.clone(),
            dry_run,
            verbose: false,
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
    run(&RunInput::PublishRegistry {
        publication: publication.clone(),
        manifest_path: fixture.manifest(),
        output: publication.clone(),
        dry_run: false,
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fs::read(publication).unwrap(), intent);
}

#[test]
#[cfg_attr(miri, ignore = "Executes the reporter with isolated workflow identity")]
fn reporter_writes_an_operator_handoff_when_publication_artifacts_are_missing() {
    let directory = TempDir::new().unwrap();
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
            "example/tool",
            "--outcomes",
        ])
        .arg(directory.path().join("missing-outcomes"))
        .arg("--publication")
        .arg(directory.path().join("missing-publication.json"))
        .arg("--jobs")
        .arg(jobs)
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
}

fn preparation(fixture: &Fixture, output: PathBuf) -> RunInput {
    RunInput::PreparePublish {
        manifest_path: fixture.manifest(),
        config: None,
        source: fixture.sha("HEAD"),
        output,
        verbose: false,
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
