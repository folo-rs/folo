//! Native command-dispatch coverage using real report files and an in-memory GitHub port.
//! The unsupported adapter injects only GitHub and a frozen clock; no credentials are read.

#![cfg(feature = "private-test-util")]

use std::env::consts::OS;
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;
use std::{fs, process, thread};

use cargo_bench_history_github::{__private, Cli};
use clap::Parser as _;
use ohno::AppError;
use serde_json::{Value, json};
use tick::Clock;
use tokio::runtime::Builder;

/// Owns report inputs for a native publication scenario, independently of the wall clock.
struct Fixture(PathBuf);

impl Fixture {
    #[expect(
        clippy::create_dir,
        reason = "Fixture collisions must fail rather than adopt existing data."
    )]
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("publication-tests")
            .join(format!(
                "{OS}-{}-{}",
                process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn cli(command: &str, args: &[&str]) -> Cli {
        Cli::try_parse_from(
            [
                "companion",
                "--repository",
                "folo-rs/folo",
                "--instance",
                "project",
                "--verbose",
                command,
            ]
            .into_iter()
            .chain(args.iter().copied()),
        )
        .unwrap()
    }

    fn report(&self, sink: &str, state: &str) -> Cli {
        let (outcome, coverage, in_scope) = match state {
            "findings" => ("findings", "full", 1),
            "clean" => ("clean", "full", 1),
            "inconclusive" => ("partial", "partial", 2),
            _ => panic!(),
        };
        let report = self.0.join(format!("{sink}-{state}.json"));
        let body = self.0.join(format!("{sink}-{state}.md"));
        fs::write(&body, format!("Tool summary: {state}")).unwrap();
        fs::write(
            &report,
            json!({
                "mode": if sink == "issue" { "history" } else { "branch" },
                "tip_commit": head(), "tip_dirty": false, "outcome": outcome,
                "notable": state == "findings",
                "census": {"coverage": coverage, "judged": 1, "in_scope": in_scope}
            })
            .to_string(),
        )
        .unwrap();
        let head = head();
        let mut args = vec![
            "--run-id",
            "42",
            "--run-attempt",
            "1",
            "--analyzed-sha",
            &head,
            "--body-file",
            body.to_str().unwrap(),
            "--report-file",
            report.to_str().unwrap(),
            "--expected-platforms",
            "linux",
            "--completed-platforms",
            "linux",
            "--artifact-url",
            "https://example.test/report-bundle",
        ];
        if sink == "comment" {
            args.extend(["--pull-request", "7", "--packages", "measured-package"]);
        }
        Self::cli(&format!("publish-{sink}-{state}"), &args)
    }

    fn status(sink: &str, state: &str) -> Cli {
        let head = head();
        let mut args = vec!["--run-id", "42", "--run-attempt", "1", "--head", &head];
        if sink == "comment" {
            args.extend(["--pull-request", "7"]);
            if state == "preflight" {
                args.extend(["--packages", "measured-package"]);
            }
        }
        match state {
            "inconclusive" => args.push("--empty-scope"),
            "failed" => args.extend([
                "--run-url",
                "https://github.com/folo-rs/folo/actions/runs/42",
                "--conclusion",
                "cancelled",
            ]),
            "preflight" => {}
            _ => panic!(),
        }
        Self::cli(&format!("publish-{sink}-{state}"), &args)
    }

    fn run(commands: Vec<Cli>, jobs: &str) -> Result<Value, AppError> {
        let output = Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(__private::run_commands(
                commands,
                Clock::new_frozen_at(SystemTime::UNIX_EPOCH),
                NonZero::new(7).unwrap(),
                &head(),
                jobs,
            ))?;
        Ok(serde_json::from_str(&output).unwrap())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let result = fs::remove_dir_all(&self.0);
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn head() -> String {
    "a".repeat(40)
}

fn body(snapshots: &Value, step: usize, sink: &str) -> String {
    snapshots
        .get(step)
        .unwrap()
        .get(sink)
        .unwrap()
        .as_array()
        .unwrap()
        .first()
        .unwrap()
        .get("body")
        .unwrap()
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
#[cfg_attr(miri, ignore = "Native report-file loading and command dispatch.")]
fn issue_commands_dispatch_their_actual_report_and_terminal_inputs() {
    let fixture = Fixture::new();
    let snapshots = Fixture::run(
        vec![
            fixture.report("issue", "findings"),
            fixture.report("issue", "clean"),
            Fixture::status("issue", "preflight"),
            fixture.report("issue", "inconclusive"),
            Fixture::status("issue", "inconclusive"),
            Fixture::status("issue", "preflight"),
            Fixture::status("issue", "failed"),
            Fixture::cli(
                "alert",
                &[
                    "--run-id",
                    "42",
                    "--run-url",
                    "https://github.com/folo-rs/folo/actions/runs/42",
                ],
            ),
        ],
        "[]",
    )
    .unwrap();
    assert_eq!(snapshots.as_array().unwrap().len(), 8);
    assert!(body(&snapshots, 0, "issues").contains("Tool summary: findings"));
    assert!(body(&snapshots, 1, "issues").contains("Tool summary: clean"));
    assert!(body(&snapshots, 2, "issues").contains(":state:preflight"));
    let partial = body(&snapshots, 3, "issues");
    assert!(partial.contains("Tool summary: inconclusive"));
    assert!(partial.contains("https://example.test/report-bundle"));
    assert!(partial.contains("Tool summary: clean"));
    assert!(body(&snapshots, 4, "issues").contains("No benchmarkable packages"));
    assert!(body(&snapshots, 5, "issues").contains(":state:preflight"));
    assert!(body(&snapshots, 6, "issues").contains("was cancelled"));
    let issues = snapshots
        .get(7)
        .unwrap()
        .get("issues")
        .unwrap()
        .as_array()
        .unwrap();
    assert_eq!(issues.len(), 2);
    assert!(
        issues
            .iter()
            .all(|issue| issue.get("open").unwrap().as_bool().unwrap())
    );
    assert!(issues.iter().any(|issue| {
        issue
            .get("title")
            .unwrap()
            .as_str()
            .unwrap()
            .ends_with("(run 42)")
    }));
    assert!(issues.iter().any(|issue| {
        issue
            .get("title")
            .unwrap()
            .as_str()
            .unwrap()
            .ends_with("(updated 1970-01-01)")
    }));
}

#[test]
#[cfg_attr(miri, ignore = "Native report-file loading and command dispatch.")]
fn comment_commands_dispatch_scope_reports_and_owned_failure() {
    let fixture = Fixture::new();
    let snapshots = Fixture::run(
        vec![
            Fixture::status("comment", "preflight"),
            fixture.report("comment", "findings"),
            fixture.report("comment", "clean"),
            fixture.report("comment", "inconclusive"),
            Fixture::status("comment", "inconclusive"),
            Fixture::status("comment", "preflight"),
            Fixture::status("comment", "failed"),
        ],
        "[]",
    )
    .unwrap();
    assert_eq!(snapshots.as_array().unwrap().len(), 7);
    assert!(body(&snapshots, 0, "comments").contains("measured-package"));
    assert!(body(&snapshots, 1, "comments").contains("Tool summary: findings"));
    assert!(body(&snapshots, 2, "comments").contains("Tool summary: clean"));
    assert!(body(&snapshots, 3, "comments").contains("Tool summary: inconclusive"));
    assert!(body(&snapshots, 4, "comments").contains("No benchmarkable package"));
    assert!(body(&snapshots, 5, "comments").contains(":in-progress"));
    assert!(body(&snapshots, 6, "comments").contains("was cancelled"));
    assert!(
        snapshots.as_array().unwrap().iter().all(|snapshot| snapshot
            .get("comments")
            .unwrap()
            .as_array()
            .unwrap()
            .len()
            == 1)
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native command loading errors use real filesystem paths."
)]
fn report_file_failures_and_evidence_mismatches_propagate_from_dispatch() {
    let fixture = Fixture::new();
    let command = fixture.report("issue", "findings");
    fs::remove_file(fixture.0.join("issue-findings.md")).unwrap();
    Fixture::run(vec![command], "[]").unwrap_err();
    let command = fixture.report("comment", "clean");
    fs::remove_file(fixture.0.join("comment-clean.json")).unwrap();
    Fixture::run(vec![command], "[]").unwrap_err();
    let command = fixture.report("issue", "inconclusive");
    fs::write(fixture.0.join("issue-inconclusive.json"), "{}").unwrap();
    Fixture::run(vec![command], "[]").unwrap_err();
    let command = fixture.report("issue", "clean");
    let file = fixture.0.join("issue-clean.json");
    let mut report: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    *report.get_mut("tip_dirty").unwrap() = json!(true);
    fs::write(file, report.to_string()).unwrap();
    Fixture::run(vec![command], "[]").unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "Native report-file loading and command dispatch.")]
fn blank_summary_files_are_rejected_for_each_report_publication_form() {
    let fixture = Fixture::new();
    for sink in ["issue", "comment"] {
        for state in ["findings", "clean", "inconclusive"] {
            for summary in ["", " \r\n\t "] {
                let command = fixture.report(sink, state);
                fs::write(fixture.0.join(format!("{sink}-{state}.md")), summary).unwrap();
                Fixture::run(vec![command], "[]").unwrap_err();
            }
        }
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native preparation dispatch creates real machine-key outputs."
)]
fn preparation_dispatch_reads_injected_jobs_and_materializes_selected_receipts() {
    let fixture = Fixture::new();
    let root = fixture.0.join("receipts").join("linux");
    fs::create_dir_all(&root).unwrap();
    fs::write(
        root.join("receipt.json"),
        json!({
            "version": 1, "repository": "folo-rs/folo", "instance": "project",
            "run_id": 42, "run_attempt": 1, "head": head(), "platform": "linux",
            "machine_key": "0123456789abcdef"
        })
        .to_string(),
    )
    .unwrap();
    let keys = fixture.0.join("keys");
    let output = fixture.0.join("output");
    let command = Fixture::cli(
        "prepare-analysis",
        &[
            "--run-id",
            "42",
            "--head",
            &head(),
            "--expected-platforms",
            "linux",
            "--receipts-dir",
            root.parent().unwrap().to_str().unwrap(),
            "--machine-key-dir",
            keys.to_str().unwrap(),
            "--github-output",
            output.to_str().unwrap(),
        ],
    );
    let jobs = json!([{
        "id": 1, "run_id": 42, "run_attempt": 1, "name": "cbh-collect:project:linux",
        "status": "completed", "conclusion": "success"
    }]);
    let snapshots = Fixture::run(vec![command], &jobs.to_string()).unwrap();
    assert!(
        snapshots
            .get(0)
            .unwrap()
            .get("issues")
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        fs::read_to_string(keys.join("linux").join("machine-key.txt")).unwrap(),
        "0123456789abcdef\n"
    );
    assert_eq!(
        fs::read_to_string(output).unwrap(),
        "completed-platforms=linux\nmachine-keys=0123456789abcdef\ncomplete=true\n"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Native integration adapter validation.")]
fn injected_adapter_rejects_bad_commands_and_bad_job_evidence() {
    let command = Fixture::cli(
        "workflow-matrix",
        &["--platforms", "linux", "--github-output", "unused"],
    );
    Fixture::run(vec![command], "[]").unwrap_err();
    let command = Fixture::status("comment", "preflight");
    Fixture::run(vec![command], "{}").unwrap_err();
    let command = Cli::try_parse_from([
        "companion",
        "alert",
        "--run-id",
        "42",
        "--run-url",
        "https://github.com/folo-rs/folo/actions/runs/42",
    ])
    .unwrap();
    Fixture::run(vec![command], "[]").unwrap_err();
}

::testing::set_allocator!();
