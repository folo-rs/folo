//! Exercises native GitHub process arguments, asset transitions and cleanup without a live forge.

#![cfg(not(miri))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::env::consts::EXE_SUFFIX;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use crp_impl::publication::binaries::{Binary, Github, Native, Outcome, execute_items};
use serde_json::json;
use tempfile::TempDir;

use crate::git_fixture::Repository;

/// Real source worktrees and an isolated `gh` implementation share no ambient test configuration.
struct Fixture {
    repository: Repository,
    forge: TempDir,
    target: String,
}

impl Fixture {
    fn new() -> Self {
        let repository = Repository::new();
        let root = repository.path();
        repository.write(
            "Cargo.toml",
            b"[workspace]\nmembers=['alpha','beta']\nresolver='3'\n",
        );
        repository.write(".gitignore", b"/target\n/out\n");
        repository.write(
            ".cargo/config.toml",
            format!("[build]\ntarget-dir='{}'\n", root.join("target").display()).as_bytes(),
        );
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        fs::copy(
            workspace.join("rust-toolchain.toml"),
            root.join("rust-toolchain.toml"),
        )
        .unwrap();
        for name in ["alpha", "beta"] {
            repository.write(
                &format!("{name}/Cargo.toml"),
                format!("[package]\nname='{name}'\nversion='1.0.0'\nedition='2024'\n").as_bytes(),
            );
            repository.write(&format!("{name}/src/main.rs"), b"fn main() {}\n");
        }
        assert_success(
            &Command::new("cargo")
                .args(["generate-lockfile", "--offline"])
                .current_dir(root)
                .output()
                .unwrap(),
        );
        repository.command(&["add", "."]);
        repository.command(&["commit", "--quiet", "-m", "native source"]);
        let compiler = Command::new("rustc")
            .args(["--version", "--verbose"])
            .output()
            .unwrap();
        assert_success(&compiler);
        let compiler = String::from_utf8(compiler.stdout).unwrap();
        let target = compiler
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap()
            .to_owned();
        let forge = TempDir::new().unwrap();
        fs::write(forge.path().join("target"), &target).unwrap();
        fs::write(forge.path().join("gh.rs"), GITHUB_PROGRAM).unwrap();
        assert_success(
            &Command::new("rustc")
                .args(["--edition=2024", "--crate-name", "fixture_gh"])
                .arg(forge.path().join("gh.rs"))
                .arg("-o")
                .arg(forge.path().join(format!("gh{EXE_SUFFIX}")))
                .output()
                .unwrap(),
        );
        Self {
            repository,
            forge,
            target,
        }
    }

    fn binaries(&self) -> Vec<Binary> {
        let source = self
            .repository
            .command(&["rev-parse", "HEAD"])
            .trim()
            .to_owned();
        ["alpha", "beta"]
            .map(|name| {
                serde_json::from_value(json!({
                    "name":name,"bin":name,"version":"1.0.0",
                    "tag":format!("{name}-v1.0.0"),"source_sha":source,
                }))
                .unwrap()
            })
            .into()
    }

    fn execute(&self, output: &str) -> Vec<Outcome> {
        let github = Github::with_executable(
            "fixture/does-not-exist".to_owned(),
            self.forge.path().join(format!("gh{EXE_SUFFIX}")),
            Some("credential-filter-canary".into()),
        );
        let mut native = Native::new(
            self.repository.path().to_path_buf(),
            self.repository.path().join("out").join(output),
            self.target.clone(),
            github,
        )
        .unwrap();
        execute_items(&self.target, &self.binaries(), false, &mut native).unwrap()
    }
}

#[test]
fn verifies_uploaded_assets_and_retries_only_incomplete_releases() {
    // Native builds and archive tools normally finish in seconds; this is only a last-chance guard.
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = Fixture::new();
        let incomplete = fixture.forge.path().join("incomplete");
        fs::write(&incomplete, "beta-v1.0.0").unwrap();
        let first = fixture.execute("first");
        assert_eq!(first.first().unwrap().status, "published");
        assert_eq!(first.last().unwrap().status, "failed");
        assert_eq!(first.last().unwrap().stage, "upload");
        fs::remove_file(incomplete).unwrap();
        let retry = fixture.execute("retry");
        assert_eq!(retry.first().unwrap().status, "skipped-complete");
        assert_eq!(retry.last().unwrap().status, "published");
        let log = fs::read_to_string(fixture.forge.path().join("calls")).unwrap();
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
fn malformed_inventory_does_not_suppress_independent_publication() {
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = Fixture::new();
        fs::write(fixture.forge.path().join("invalid"), "alpha-v1.0.0").unwrap();
        let outcomes = fixture.execute("malformed");
        assert_eq!(outcomes.first().unwrap().status, "failed");
        assert_eq!(outcomes.first().unwrap().stage, "refresh");
        assert_eq!(outcomes.last().unwrap().status, "published");
    });
}

#[test]
fn cleanup_failure_preserves_successful_publication() {
    testing::with_watchdog_timeout(Duration::from_mins(5), || {
        let fixture = Fixture::new();
        // Locking the owned Git worktree deterministically prevents ordinary removal.
        fixture.repository.write(
            "alpha/build.rs",
            br#"
use std::{env, path::PathBuf, process::Command};
fn main() {
    let package = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    assert!(Command::new("git").args(["worktree", "lock", "--reason", "cleanup fixture"])
        .arg(package.parent().unwrap()).status().unwrap().success());
}
"#,
        );
        fixture.repository.command(&["add", "."]);
        fixture
            .repository
            .command(&["commit", "--quiet", "-m", "locked cleanup"]);
        let outcomes = fixture.execute("cleanup");
        assert!(outcomes.iter().all(|item| item.status == "published"));
        assert!(outcomes.iter().all(|item| item.cleanup_error.is_some()));
        assert!(fixture.forge.path().join("alpha-v1.0.0").is_file());
    });
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// The native protocol fixture observes the production command boundary, not an in-process fake.
// Its own directory supplies state, avoiding process-global environment edits in parallel tests.
const GITHUB_PROGRAM: &str = r#"
use std::{env, fs::{self, OpenOptions}, io::Write, path::PathBuf};
fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    assert_eq!(args[0], "release");
    assert_eq!(env::var("GH_TOKEN").unwrap(), "credential-filter-canary");
    for name in ["GITHUB_TOKEN", "GIT_TOKEN", "INPUT_TOKEN", "DEFAULT_GITHUB_TOKEN"] {
        assert!(env::var_os(name).is_none());
    }
    assert_eq!(&args[args.len()-2..], ["--repo", "fixture/does-not-exist"]);
    let directory = env::current_exe().unwrap().parent().unwrap().to_path_buf();
    let tag = &args[2];
    let mut log = OpenOptions::new().create(true).append(true).open(directory.join("calls")).unwrap();
    writeln!(log, "{} {tag}", args[1]).unwrap();
    let base = format!("{tag}-{}", fs::read_to_string(directory.join("target")).unwrap());
    let state = directory.join(tag);
    match args[1].as_str() {
        "view" => {
            assert_eq!(&args[3..5], ["--json", "assets"]);
            if fs::read_to_string(directory.join("invalid")).ok().as_deref() == Some(tag.as_str()) {
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
            if fs::read_to_string(directory.join("incomplete")).ok().as_deref() != Some(tag.as_str()) {
                fs::write(state, "").unwrap();
            }
        }
        _ => panic!("unexpected GitHub operation"),
    }
}
"#;
