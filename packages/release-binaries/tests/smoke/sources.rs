//! Source acquisition and preparation failures use disposable repositories, never remote services.

use std::env::consts::EXE_SUFFIX;
use std::fs;

use serde_json::{Value, json};

use crate::{Fixture, SMOKE_WATCHDOG, assert_success, command, run, write};

#[test]
fn fetches_the_exact_missing_source_without_using_the_remote_tip() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let fixture = Fixture::new();
        let mut remote = Fixture::new();
        write(
            remote.root.path(),
            ".cargo/config.toml",
            "[env]\nRELEASE_FIXTURE_CONFIG = \"fetched\"\n",
        );
        remote.commit_source();
        let released = remote.binary("beta");
        write(
            remote.root.path(),
            ".cargo/config.toml",
            "[env]\nRELEASE_FIXTURE_CONFIG = \"unreleased\"\n",
        );
        remote.commit_source();
        run(
            fixture.root.path(),
            "git",
            &[
                "remote",
                "add",
                "origin",
                remote.root.path().to_str().unwrap(),
            ],
        );
        let controller = run(fixture.root.path(), "git", &["rev-parse", "HEAD"]);
        let missing = command(fixture.root.path(), "git")
            .args(["cat-file", "-e", released["source_sha"].as_str().unwrap()])
            .output()
            .unwrap();
        assert!(!missing.status.success());

        let result = fixture.execute(&json!([fixture.binary("alpha"), released]), "out");
        assert_success(&result);
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes.as_array().unwrap().len(), 2);
        for (name, expected, source) in [
            ("alpha", "tagged", fixture.source.as_str()),
            ("beta", "fetched", released["source_sha"].as_str().unwrap()),
        ] {
            let outcome = outcomes
                .as_array()
                .unwrap()
                .iter()
                .find(|outcome| outcome["binary"]["name"] == name)
                .unwrap();
            assert_eq!(outcome["status"], "staged-only");
            assert_eq!(outcome["binary"]["source_sha"], source);
            let staged = fixture
                .root
                .path()
                .join("out")
                .join(format!("{name}-v1.0.0-{}", fixture.triple));
            assert_eq!(
                run(&staged, staged.join(format!("{name}-bin{EXE_SUFFIX}")), &[]).trim(),
                expected
            );
        }
        assert_eq!(
            run(fixture.root.path(), "git", &["rev-parse", "HEAD"]),
            controller
        );
        assert_eq!(
            run(
                fixture.root.path(),
                "git",
                &["worktree", "list", "--porcelain"]
            )
            .matches("worktree ")
            .count(),
            1
        );
    });
}

#[test]
fn rejects_a_foreign_compiler_host_and_cleans_the_prepared_source() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        // Keep this negative scenario offline: successful installation is not evidence
        // that rustc's actual host matches the requested native target.
        write(
            fixture.root.path(),
            "scripts/release/Install-ReleaseSourceToolchain.ps1",
            "param([string] $Target)\n",
        );
        assert_success(&fixture.execute(&json!([fixture.binary("alpha")]), "out/native-host"));
        fixture.triple = if cfg!(windows) {
            "x86_64-unknown-linux-gnu"
        } else {
            "x86_64-pc-windows-msvc"
        }
        .to_owned();
        let result = fixture.execute(&json!([fixture.binary("alpha")]), "out");
        assert!(!result.status.success());
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes.as_array().unwrap().len(), 1);
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[0]["stage"], "source");
        assert!(outcomes[0]["cleanup_error"].is_null());
        assert_eq!(
            run(
                fixture.root.path(),
                "git",
                &["worktree", "list", "--porcelain"]
            )
            .matches("worktree ")
            .count(),
            1
        );
    });
}
