//! Executable GitHub adapter coverage without any network request or real release mutation.

use std::path::PathBuf;
use std::process::Command;
use std::{env, fs};

use serde_json::{Value, json};

use crate::{Fixture, SMOKE_WATCHDOG, assert_success, command, compile_tool, write};

/// A native fake `gh` executable keeps process arguments, credentials and release transitions
/// observable while exercising the production controller without GitHub access.
struct GithubFixture {
    directory: PathBuf,
}

impl GithubFixture {
    fn new(fixture: &Fixture) -> Self {
        let directory = fixture.root.path().join("github-fixture");
        fs::create_dir_all(&directory).unwrap();
        compile_tool(
            &directory,
            "gh",
            r#"
use std::env;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args[0], "release");
    assert_eq!(env::var("GH_TOKEN").unwrap(), "credential-filter-canary");
    for name in ["GITHUB_TOKEN", "GIT_TOKEN", "INPUT_TOKEN", "DEFAULT_GITHUB_TOKEN"] {
        assert!(env::var_os(name).is_none());
    }
    assert_eq!(&args[args.len() - 2..], ["--repo", "fixture/does-not-exist"]);
    let directory = PathBuf::from(env::var_os("RELEASE_FIXTURE_GITHUB").unwrap());
    let tag = &args[2];
    let mut log = OpenOptions::new().create(true).append(true).open(directory.join("calls")).unwrap();
    writeln!(log, "{} {tag}", args[1]).unwrap();
    let base = format!("{tag}-{}", env::var("RELEASE_FIXTURE_TRIPLE").unwrap());
    let state = directory.join(tag);
    match args[1].as_str() {
        "view" => {
            assert_eq!(&args[3..5], ["--json", "assets"]);
            if env::var("RELEASE_FIXTURE_INVALID_JSON").as_deref() == Ok(tag) {
                println!("invalid JSON");
            } else if state.is_file() {
                println!("{{\"assets\":[{{\"name\":\"{base}.zip\",\"state\":\"uploaded\"}},{{\"name\":\"{base}.sha256\",\"state\":\"uploaded\"}}]}}");
            } else {
                println!("{{\"assets\":[]}}");
            }
        }
        "upload" => {
            for (index, extension) in [(3, "zip"), (4, "sha256")] {
                let file = PathBuf::from(&args[index]);
                assert_eq!(file.file_name().unwrap(), format!("{base}.{extension}").as_str());
                assert!(file.is_file());
            }
            assert_eq!(args[5], "--clobber");
            if env::var("RELEASE_FIXTURE_INCOMPLETE_UPLOAD").as_deref() != Ok(tag) {
                fs::write(state, "").unwrap();
            }
        }
        _ => panic!("unexpected GitHub operation"),
    }
}
"#,
        );
        Self { directory }
    }

    fn configure(&self, fixture: &Fixture, command: &mut Command) {
        let path = env::join_paths(
            std::iter::once(self.directory.clone())
                .chain(env::split_paths(&env::var_os("PATH").unwrap())),
        )
        .unwrap();
        command
            .env("PATH", path)
            .env("RELEASE_FIXTURE_GITHUB", &self.directory)
            .env("RELEASE_FIXTURE_TRIPLE", &fixture.triple)
            .env("GH_TOKEN", "credential-filter-canary");
    }
}

#[test]
fn verifies_uploaded_assets_and_retries_only_incomplete_releases() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let fixture = Fixture::new();
        let github = GithubFixture::new(&fixture);
        let binaries = json!([fixture.binary("alpha"), fixture.binary("beta")]);
        let summary = fixture.root.path().join("summary.md");
        let mut first = fixture.batch_command(&binaries, "out");
        github.configure(&fixture, &mut first);
        // A successful CLI exit alone cannot establish successful publication.
        first
            .env("RELEASE_FIXTURE_INCOMPLETE_UPLOAD", "beta-v1.0.0")
            .env("GITHUB_STEP_SUMMARY", &summary);
        let result = first.output().unwrap();
        assert!(!result.status.success());
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes[0]["status"], "published");
        assert_eq!(outcomes[1]["status"], "failed");
        assert_eq!(outcomes[1]["stage"], "upload");
        assert!(fs::read_to_string(summary).unwrap().contains("published"));

        let mut requests = binaries.as_array().unwrap().clone();
        for request in &mut requests {
            request["release_targets"] = json!([]);
        }
        let plan_input = fixture.root.path().join("plan.json");
        fs::write(
            &plan_input,
            serde_json::to_vec(&json!({
                "binaries": requests,
                "targets": [{"triple": fixture.triple, "os": "fixture"}],
            }))
            .unwrap(),
        )
        .unwrap();
        let mut plan = command(fixture.root.path(), env!("CARGO_BIN_EXE_release-binaries"));
        plan.args(["plan", "--repository", "fixture/does-not-exist", "--input"])
            .arg(plan_input);
        github.configure(&fixture, &mut plan);
        let result = plan.output().unwrap();
        assert_success(&result);
        let batches: Value = serde_json::from_slice(&result.stdout).unwrap();
        assert_eq!(batches.as_array().unwrap().len(), 1);
        assert_eq!(batches[0]["binaries"].as_array().unwrap().len(), 1);
        assert_eq!(batches[0]["binaries"][0]["name"], "beta");

        let mut retry = fixture.batch_command(&binaries, "out/retry");
        github.configure(&fixture, &mut retry);
        assert_success(&retry.output().unwrap());
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/retry/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes[0]["status"], "skipped-complete");
        assert_eq!(outcomes[1]["status"], "published");
        let log = fs::read_to_string(github.directory.join("calls")).unwrap();
        assert_eq!(
            log.lines()
                .filter(|line| *line == "upload alpha-v1.0.0")
                .count(),
            1
        );
        assert_eq!(
            log.lines()
                .filter(|line| *line == "upload beta-v1.0.0")
                .count(),
            2
        );
    });
}

#[test]
fn malformed_asset_inventory_fails_the_item_without_suppressing_other_releases() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let fixture = Fixture::new();
        let github = GithubFixture::new(&fixture);
        let mut command = fixture.batch_command(
            &json!([fixture.binary("alpha"), fixture.binary("beta")]),
            "out",
        );
        github.configure(&fixture, &mut command);
        command.env("RELEASE_FIXTURE_INVALID_JSON", "alpha-v1.0.0");
        assert!(!command.output().unwrap().status.success());
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[0]["stage"], "refresh");
        assert_eq!(outcomes[1]["status"], "published");
    });
}

#[test]
fn cleanup_failure_preserves_successful_publication() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        // A locked worktree refuses ordinary removal, independently of successful compilation.
        // Both the registration and its source directory belong to this disposable fixture.
        write(
            fixture.root.path(),
            "alpha/build.rs",
            r#"
use std::env;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    assert!(Command::new("git")
        .args(["worktree", "lock", "--reason", "cleanup fixture"])
        .arg(package.parent().unwrap())
        .status().unwrap().success());
}
"#,
        );
        fixture.commit_source();
        let github = GithubFixture::new(&fixture);
        let mut command = fixture.batch_command(&json!([fixture.binary("alpha")]), "out");
        github.configure(&fixture, &mut command);
        let result = command.output().unwrap();
        assert!(!result.status.success());
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes.as_array().unwrap().len(), 1);
        assert_eq!(
            outcomes[0]["status"],
            "published",
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(outcomes[0]["cleanup_error"].is_string());
        assert!(github.directory.join("alpha-v1.0.0").is_file());
    });
}
