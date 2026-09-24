//! Native Git, package discovery and historical-range preparation for reusable workflows.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

/// Owns an isolated workspace plus event/config/output files outside the measured checkout.
struct Fixture {
    root: TempDir,
    checkout: PathBuf,
    base: String,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("measured");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            checkout.join("Cargo.toml"),
            "[workspace]\nresolver='3'\nmembers=['library','benchmark','unrelated']\n",
        )
        .unwrap();
        for (name, bench, dependency) in [
            ("library", false, false),
            ("benchmark", true, true),
            ("unrelated", true, false),
        ] {
            let directory = checkout.join(name);
            fs::create_dir_all(directory.join("src")).unwrap();
            fs::write(directory.join("src").join("lib.rs"), "").unwrap();
            let mut manifest =
                format!("[package]\nname='{name}'\nversion='0.0.0'\nedition='2024'\n");
            if bench {
                manifest.push_str("[[bench]]\nname='probe'\nharness=false\n");
                fs::create_dir_all(directory.join("benches")).unwrap();
                fs::write(directory.join("benches").join("probe.rs"), "fn main() {}\n").unwrap();
            }
            if dependency {
                manifest.push_str("[dev-dependencies]\nlibrary={path='../library'}\n");
            }
            fs::write(directory.join("Cargo.toml"), manifest).unwrap();
        }
        fs::write(checkout.join("library").join("removed.rs"), "old source\n").unwrap();
        let output = Command::new("cargo")
            .current_dir(&checkout)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut fixture = Self {
            root,
            checkout,
            base: String::new(),
            head: String::new(),
        };
        fixture.git(&["init", "--quiet"]);
        fixture.git(&["add", "."]);
        fixture.commit();
        fixture
            .git(&["rev-parse", "HEAD"])
            .trim()
            .clone_into(&mut fixture.base);
        fs::remove_file(fixture.checkout.join("library").join("removed.rs")).unwrap();
        fixture.git(&["add", "-u"]);
        fixture.commit();
        fixture
            .git(&["rev-parse", "HEAD"])
            .trim()
            .clone_into(&mut fixture.head);
        fixture
    }

    fn git(&self, args: &[&str]) -> String {
        self.git_at(args, "2024-03-01T12:00:00Z")
    }

    fn git_at(&self, args: &[&str], date: &str) -> String {
        let output = Command::new("git")
            .current_dir(&self.checkout)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn commit(&self) {
        // Initial commits precede the rolling tests' March lookback window.
        self.commit_at("2024-03-01T12:00:00Z");
    }

    fn commit_at(&self, date: &str) {
        self.git_at(
            &[
                "-c",
                "user.name=Preparation Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "fixture",
            ],
            date,
        );
    }

    fn command(&self, flow: &str, input: &Value) -> Command {
        let input_path = self.root.path().join("inputs.json");
        let event_path = self.root.path().join("event.json");
        fs::write(&input_path, serde_json::to_vec(input).unwrap()).unwrap();
        fs::write(
            &event_path,
            serde_json::to_vec(&json!({
                "number":7, "pull_request":{
                    "head":{"sha":self.head, "repo":{"full_name":"owner/repo"}},
                    "base":{"sha":self.base, "repo":{"full_name":"owner/repo"}}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-github"));
        command
            .current_dir(&self.checkout)
            .args(["prepare-workflow", "--flow", flow, "--inputs-file"])
            .arg(input_path)
            .arg("--github-output")
            .arg(self.root.path().join("outputs"));
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "GITHUB_EVENT_NAME",
            "GITHUB_EVENT_PATH",
            "GITHUB_REPOSITORY",
            "GITHUB_SHA",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
        ] {
            command.env_remove(name);
        }
        if flow == "pr" {
            command
                .env("GITHUB_EVENT_NAME", "pull_request")
                .env("GITHUB_EVENT_PATH", event_path)
                .env("GITHUB_SHA", "c".repeat(40));
        }
        command
    }

    fn prepare(&self, flow: &str, input: &Value) -> BTreeMap<String, String> {
        let output = self.command(flow, input).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = fs::read_to_string(self.root.path().join("outputs")).unwrap();
        text.lines()
            .map(|line| {
                let (key, value) = line.split_once('=').unwrap();
                (key.to_owned(), value.to_owned())
            })
            .collect()
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native Git, Cargo metadata, process and filesystem boundaries."
)]
fn native_history_workspace_and_pr_affected_deleted_file_scopes() {
    let fixture = Fixture::new();
    let config = fixture.root.path().join("authority.toml");
    fs::write(&config, "[project]\nid='Measured Project!'").unwrap();
    let mut input = json!({"platforms":"windows,linux", "config":config});
    let outputs = fixture.prepare("history", &input);
    assert_eq!(outputs.get("instance").unwrap(), "measured_project_");
    assert_eq!(outputs.get("packages").unwrap(), "benchmark,unrelated");
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
    assert_eq!(outputs.get("base").unwrap(), &fixture.head);
    assert_eq!(outputs.get("skip-all").unwrap(), "false");
    assert_eq!(
        outputs.get("collection-job-prefix").unwrap(),
        "cbh-collect:measured_project_"
    );
    _ = input
        .as_object_mut()
        .unwrap()
        .insert("exclude".to_owned(), json!("library"));
    let outputs = fixture.prepare("pr", &input);
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
    assert_eq!(outputs.get("base").unwrap(), &fixture.base);
    assert_eq!(outputs.get("packages").unwrap(), "benchmark");
    assert_eq!(outputs.get("skip-all").unwrap(), "false");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native Git, Cargo metadata, process and filesystem boundaries."
)]
fn native_empty_scope_and_history_freeze_have_concrete_outputs() {
    let fixture = Fixture::new();
    let outputs = fixture.prepare(
        "history",
        &json!({
            "platforms":"linux", "exclude":"benchmark,unrelated",
        }),
    );
    assert_eq!(outputs.get("packages").unwrap(), "");
    assert_eq!(outputs.get("skip-all").unwrap(), "true");
    assert_eq!(outputs.get("head"), outputs.get("base"));
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native checkout mismatch and append-only output boundary."
)]
fn native_bad_event_head_leaves_existing_outputs_untouched() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("outputs"), "previous=value\n").unwrap();
    let output = fixture
        .command("history", &json!({"platforms":"linux"}))
        .env("GITHUB_SHA", &fixture.base)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("outputs")).unwrap(),
        "previous=value\n"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Native Git and filesystem preparation boundary.")]
fn native_backfill_freezes_symbolic_endpoints_without_current_package_scope() {
    let fixture = Fixture::new();
    let config = fixture.root.path().join("authority.toml");
    fs::write(&config, "[project]\nid='Historical Project'").unwrap();
    let outputs = fixture.prepare(
        "backfill",
        &json!({
            "platforms":"windows,linux", "config":config,
            "from":"HEAD~1", "to":"HEAD", "exclude":"historical-only",
            "lookback":"", "minimum-age":"",
        }),
    );
    assert_eq!(outputs.get("instance").unwrap(), "historical_project");
    assert_eq!(outputs.get("from").unwrap(), &fixture.base);
    assert_eq!(outputs.get("to").unwrap(), &fixture.head);
    assert_eq!(outputs.get("skipped").unwrap(), "false");
    assert_eq!(outputs.get("has-work").unwrap(), "true");
    for field in [
        "head",
        "base",
        "packages",
        "skip-all",
        "collection-job-prefix",
    ] {
        assert!(!outputs.contains_key(field));
    }
}

#[test]
#[cfg_attr(miri, ignore = "Native Git and historical repository contents.")]
fn native_backfill_does_not_require_cargo_metadata_at_the_invocation_head() {
    let fixture = Fixture::new();
    fixture.git(&["rm", "--quiet", "Cargo.toml", "Cargo.lock"]);
    fixture.commit();
    let config = fixture.root.path().join("authority.toml");
    fs::write(&config, "[project]\nid='Historical Project'").unwrap();
    let outputs = fixture.prepare(
        "backfill",
        &json!({
            "platforms":"linux", "config":config,
            "from":fixture.base, "to":fixture.head,
        }),
    );
    assert_eq!(outputs.get("from").unwrap(), &fixture.base);
    assert_eq!(outputs.get("to").unwrap(), &fixture.head);
    assert_eq!(outputs.get("skipped").unwrap(), "false");
    assert_eq!(outputs.get("has-work").unwrap(), "true");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native Cargo workspace boundaries and Git change discovery."
)]
fn native_independent_virtual_workspaces_do_not_become_foreign_scope_owners() {
    for location in [".github/fixture", "library/tests/fixture"] {
        let mut fixture = Fixture::new();
        let nested = fixture.checkout.join(location);
        fs::create_dir_all(nested.join("foreign").join("src")).unwrap();
        fs::write(
            nested.join("Cargo.toml"),
            "[workspace]\nmembers=['foreign']\nresolver='3'\n",
        )
        .unwrap();
        fs::write(
            nested.join("foreign").join("Cargo.toml"),
            "[package]\nname='foreign'\nversion='0.0.0'\nedition='2024'\n",
        )
        .unwrap();
        fs::write(nested.join("foreign").join("src").join("lib.rs"), "").unwrap();
        fixture.git(&["add", "."]);
        fixture.commit();
        fixture.head = fixture.git(&["rev-parse", "HEAD"]).trim().to_owned();
        let outputs = fixture.prepare("pr", &json!({"platforms":"linux"}));
        let expected = if location.starts_with(".github") {
            "benchmark,unrelated"
        } else {
            "benchmark"
        };
        assert_eq!(outputs.get("packages").unwrap(), expected);
        assert_eq!(outputs.get("skip-all").unwrap(), "false");
    }
}

#[cfg(feature = "private-test-util")]
mod rolling {
    use std::time::SystemTime;

    use cargo_bench_history_github::__private::prepare_backfill_at;
    use jiff::Timestamp;
    use tick::Clock;

    use super::*;

    fn commit(fixture: &Fixture, date: &str) -> String {
        fixture.commit_at(date);
        fixture.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    async fn range(fixture: &Fixture, input: Value, now: &str) -> Option<(String, String)> {
        let clock = Clock::new_frozen_at(SystemTime::from(now.parse::<Timestamp>().unwrap()));
        prepare_backfill_at(
            &serde_json::to_vec(&input).unwrap(),
            &fixture.checkout,
            fixture.git(&["rev-parse", "HEAD"]).trim(),
            &clock,
        )
        .await
        .unwrap()
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "real Git traversal and filesystem fixture")]
    async fn native_rolling_cutoffs_and_override_use_the_frozen_now() {
        let fixture = Fixture::new();
        commit(&fixture, "2024-03-16T12:00:00Z");
        let from = commit(&fixture, "2024-03-17T12:00:00Z");
        let eligible = commit(&fixture, "2024-03-30T12:00:00Z");
        let recent = commit(&fixture, "2024-03-31T12:00:00Z");
        let mut input = json!({
            "platforms":"linux", "lookback":"14 days", "minimum-age":"24 hours",
            "from":"", "to":"",
        });
        assert_eq!(
            range(&fixture, input.clone(), "2024-03-31T12:00:00Z").await,
            Some((from.clone(), eligible))
        );
        _ = input
            .as_object_mut()
            .unwrap()
            .insert("to".to_owned(), json!("HEAD"));
        assert_eq!(
            range(&fixture, input, "2024-03-31T12:00:00Z").await,
            Some((from, recent.clone()))
        );
        assert_eq!(fixture.git(&["rev-parse", "HEAD"]).trim(), recent);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "real Git traversal and same-second commit timestamps")]
    async fn native_rolling_subsecond_boundaries_preserve_commit_eligibility() {
        let fixture = Fixture::new();
        let earlier = commit(&fixture, "2024-03-31T12:00:00Z");
        let latest = commit(&fixture, "2024-03-31T12:00:00Z");
        // The frozen fraction is representable by Windows SystemTime as well as Unix clocks.
        let now = "2024-03-31T12:00:00.000000100Z";
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"100ns", "minimum-age":"0 seconds"}),
                now,
            )
            .await,
            Some((earlier, latest.clone()))
        );
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"1ns", "minimum-age":"0 seconds"}),
                now,
            )
            .await,
            Some((latest.clone(), latest))
        );
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"1ns", "minimum-age":"200ns"}),
                now,
            )
            .await,
            Some((fixture.head.clone(), fixture.head.clone()))
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "real Git traversal and filesystem fixture")]
    async fn native_rolling_no_eligible_endpoint_is_no_work() {
        let fixture = Fixture::new();
        assert_eq!(
            range(
                &fixture,
                json!({
                    "platforms":"linux", "lookback":"14 days", "minimum-age":"0 seconds",
                    "from":"", "to":"",
                }),
                "2024-02-29T12:00:00Z",
            )
            .await,
            None
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "real Git traversal and filesystem fixture")]
    async fn native_rolling_old_history_falls_back_to_one_commit() {
        let fixture = Fixture::new();
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"14 days", "minimum-age":"1 day"}),
                "2024-03-31T12:00:00Z",
            )
            .await,
            Some((fixture.head.clone(), fixture.head.clone()))
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore = "real Git traversal with pre-epoch calendar cutoffs")]
    async fn native_rolling_calendar_bounds_do_not_use_git_approximate_dates() {
        let fixture = Fixture::new();
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"100 years", "minimum-age":"1 day"}),
                "2026-09-22T08:00:00Z",
            )
            .await,
            Some((fixture.base.clone(), fixture.head.clone()))
        );
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"100 years", "minimum-age":"100 years"}),
                "2026-09-22T08:00:00Z",
            )
            .await,
            None
        );
    }

    #[tokio::test]
    #[cfg_attr(
        miri,
        ignore = "real Git first-parent traversal with nonmonotonic commit dates"
    )]
    async fn native_rolling_preserves_first_parent_and_git_date_filtering() {
        let fixture = Fixture::new();
        let branch = fixture.git(&["branch", "--show-current"]);
        commit(&fixture, "2024-03-24T12:00:00Z");
        // Git's --since walk stops at this old ancestor rather than visiting every commit.
        commit(&fixture, "2024-03-10T12:00:00Z");
        let mainline = commit(&fixture, "2024-03-25T12:00:00Z");
        fixture.git(&["checkout", "--quiet", "-b", "side", &fixture.head]);
        commit(&fixture, "2024-03-29T12:00:00Z");
        fixture.git(&["checkout", "--quiet", branch.trim()]);
        fixture.git_at(
            &[
                "-c",
                "user.name=Preparation Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-c",
                "commit.gpgsign=false",
                "merge",
                "--quiet",
                "--no-ff",
                "-m",
                "merge side",
                "side",
            ],
            "2024-03-31T12:00:00Z",
        );
        assert_eq!(
            range(
                &fixture,
                json!({"platforms":"linux", "lookback":"14 days", "minimum-age":"1 day"}),
                "2024-03-31T12:00:00Z",
            )
            .await,
            Some((mainline.clone(), mainline))
        );
    }
}

::testing::set_allocator!();
