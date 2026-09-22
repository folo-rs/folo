use std::ffi::OsString;

use futures::executor::block_on;
use serde_json::{Value, json};

use crate::action::execute::run_with;
use crate::action::port::Output;
use crate::action::tests::fake::{FakeHost, FakePublisher, SHA, analysis};

#[test]
fn collect_streams_then_captures_only_machine_key_and_appends_after_success() {
    let mut host = FakeHost::new(&json!({
        "command":"collect", "working-directory":"checkout", "config":"custom.toml",
        "local-path":"store", "packages":"crate-a,crate-b", "bench":"speed,allocations",
        "features":"a,b", "all-features":"false", "no-default-features":"true",
        "best-of":"3", "on-existing":"skip"
    }));
    host.files.insert(
        host.root.join("checkout").join("custom.toml"),
        b"[project]\nid = 'My Project'\n".to_vec(),
    );
    host.reply("");
    host.reply("0123456789ABCDEF\n");
    let mut args = host.args();
    args.tool = Some("tools\\core.exe".into());
    let publisher = FakePublisher::default();
    block_on(run_with(args, &host, &publisher)).unwrap();
    let processes = host.processes.borrow();
    let collect = processes.first().unwrap();
    let cwd = host.root.join("checkout");
    assert_eq!(collect.cwd, cwd);
    assert_eq!(collect.program, cwd.join("tools\\core.exe"));
    assert_eq!(collect.output, Output::Inherit);
    assert!(collect.env.is_empty());
    let expected: Vec<OsString> = vec![
        "collect".into(),
        "--verbose".into(),
        format!("--config={}", cwd.join("custom.toml").display()).into(),
        format!("--local={}", cwd.join("store").display()).into(),
        "--package=crate-a".into(),
        "--package=crate-b".into(),
        "--bench=speed".into(),
        "--bench=allocations".into(),
        "--features=a".into(),
        "--features=b".into(),
        "--best-of=3".into(),
        "--no-default-features".into(),
        "--skip-existing".into(),
    ];
    assert_eq!(collect.args, expected);
    let key = processes.get(1).unwrap();
    assert_eq!(key.output, Output::Capture);
    assert_eq!(key.args, [OsString::from("machine-key")]);
    assert_eq!(key.cwd, cwd);
    assert!(key.env.is_empty());
    assert_eq!(
        *host.outputs.borrow(),
        [(
            cwd.join("github-output"),
            "instance=my_project\nmachine-key=0123456789abcdef\n".to_owned()
        )]
    );
    assert!(publisher.commands.borrow().is_empty());
    assert!(
        !host
            .environment_reads
            .borrow()
            .iter()
            .any(|name| matches!(name.as_str(), "GITHUB_TOKEN" | "GH_TOKEN"))
    );
}

#[test]
fn build_defaults_exclusions_and_write_modes_are_per_command() {
    for (command, mode, flag) in [
        ("collect", "", None),
        ("collect", "error", None),
        ("collect", "overwrite", Some("--overwrite")),
        ("backfill", "", None),
        ("backfill", "skip", None),
        ("backfill", "overwrite", Some("--overwrite")),
    ] {
        let mut input = json!({"command":command, "exclude":"omit,other", "on-existing":mode});
        if command == "backfill" {
            input["from"] = json!("HEAD~2");
            input["to"] = json!("HEAD");
            input["ignore-errors"] = json!("true");
        }
        let host = FakeHost::new(&input);
        host.reply("");
        if command == "collect" {
            host.reply("0123456789abcdef");
        }
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
        let processes = host.processes.borrow();
        let args = &processes.first().unwrap().args;
        assert!(args.contains(&"--workspace".into()));
        assert!(args.contains(&"--exclude=omit".into()));
        assert!(args.contains(&"--exclude=other".into()));
        assert!(args.contains(&"--all-features".into()));
        assert!(args.contains(&"--best-of=1".into()));
        assert!(!args.contains(&"--skip-existing".into()));
        assert!(
            !args
                .iter()
                .any(|arg| arg.to_string_lossy().starts_with("--config"))
        );
        assert_eq!(args.contains(&"--overwrite".into()), flag.is_some());
        if command == "backfill" {
            assert!(args.contains(&"HEAD~2".into()));
            assert!(args.contains(&"HEAD".into()));
            assert!(args.contains(&"--ignore-errors".into()));
            assert_eq!(processes.len(), 1);
            assert_eq!(
                host.outputs.borrow().first().unwrap().1,
                "instance=action-test\n"
            );
        }
    }
}

#[test]
fn collection_and_machine_key_failures_withhold_all_outputs() {
    for failure in ["collect", "key", "invalid-key"] {
        let host = FakeHost::new(&json!({"command":"collect"}));
        if failure == "collect" {
            host.fail();
        } else {
            host.reply("");
            if failure == "key" {
                host.fail();
            } else {
                host.reply("all");
            }
        }
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
        assert!(host.outputs.borrow().is_empty());
        assert_eq!(
            host.processes.borrow().len(),
            if failure == "collect" { 1 } else { 2 }
        );
    }
}

#[test]
fn history_passes_resolved_context_as_base_and_explicit_actual_keys() {
    let mut input = analysis("analyze-history");
    input["context"] = json!("release");
    input["since"] = json!("30d");
    input["cache"] = json!("../cache");
    let mut host = FakeHost::new(&input);
    // Use an absolute cache in this pure fixture; the native test covers parent traversal.
    input["cache"] = json!(host.root.join("cache"));
    host.files.insert(
        host.root.join("inputs.json"),
        serde_json::to_vec(&input).unwrap(),
    );
    host.analysis_replies();
    host.reports("history", "findings", "full", 1, 1);
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
    let processes = host.processes.borrow();
    assert!(processes.iter().all(|process| process.env.is_empty()));
    assert_eq!(
        processes.get(2).unwrap().args.last().unwrap(),
        "release^{commit}"
    );
    let process = processes.last().unwrap();
    assert_eq!(process.output, Output::Inherit);
    for arg in [
        "--engine=all",
        "--target-triple=all",
        "--no-dirty",
        "--no-text",
        "--verbose",
        "--machine-key=0123456789abcdef",
        "--since=30d",
    ] {
        assert!(process.args.contains(&arg.into()));
    }
    assert!(process.args.contains(&format!("--context={SHA}").into()));
    assert!(process.args.contains(&format!("--base={SHA}").into()));
    assert_eq!(
        process
            .args
            .iter()
            .filter(|arg| arg.to_string_lossy().starts_with("--machine-key="))
            .count(),
        1
    );
    let outputs = host.outputs.borrow().first().unwrap().1.clone();
    for value in [
        "instance=checkout\n",
        "outcome=findings\n",
        "notable=true\n",
        "partial-platform-coverage=true\n",
        "regressions=1\n",
        "publication-state=findings\n",
        "report-markdown=",
        "report-json=",
        "report-summary=",
    ] {
        assert!(outputs.contains(value));
    }
    assert_eq!(host.scratches.borrow().as_slice(), [host.root.join("temp")]);
    assert_eq!(
        host.key_roots.borrow().as_slice(),
        [host.root.join("checkout").join("keys")]
    );
}

#[test]
fn pr_uses_explicit_base_or_core_default_without_fabricating_a_branch_name() {
    for base in [None, Some("origin/release")] {
        let mut input = analysis("analyze-pr");
        if let Some(base) = base {
            input["base"] = json!(base);
        }
        let mut host = FakeHost::new(&input);
        host.analysis_replies();
        host.reports("branch", "clean", "full", 1, 1);
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
        let processes = host.processes.borrow();
        let args = &processes.last().unwrap().args;
        assert_eq!(
            args.iter()
                .find(|arg| arg.to_string_lossy().starts_with("--base=")),
            base.map(|base| OsString::from(format!("--base={base}")))
                .as_ref()
        );
        let outputs = host.outputs.borrow().first().unwrap().1.clone();
        assert!(outputs.contains("publication-state=inconclusive\n"));
        assert!(outputs.contains("can-clear=false\n"));
    }
}

fn assert_outcome(outcome: &str, coverage: &str, judged: usize, in_scope: usize, state: &str) {
    let mut input = analysis("analyze-history");
    input["completed-platforms"] = json!("windows,linux");
    let mut host = FakeHost::new(&input);
    host.analysis_replies();
    host.reports("history", outcome, coverage, judged, in_scope);
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
    let outputs = host.outputs.borrow().first().unwrap().1.clone();
    assert!(outputs.contains(&format!("outcome={outcome}\n")));
    assert!(outputs.contains(&format!("publication-state={state}\n")));
    assert!(outputs.contains("partial-platform-coverage=false\n"));
    assert!(outputs.contains(&format!("can-clear={}\n", outcome == "clean")));
}

#[test]
fn clean_history_uses_shared_evidence_projection() {
    assert_outcome("clean", "full", 1, 1, "clean");
}

#[test]
fn partial_history_uses_shared_evidence_projection() {
    assert_outcome("partial", "partial", 1, 2, "inconclusive");
}

#[test]
fn insufficient_history_uses_shared_evidence_projection() {
    assert_outcome(
        "insufficient_baseline",
        "nothing_judged",
        0,
        1,
        "inconclusive",
    );
}

#[test]
fn empty_history_uses_shared_evidence_projection() {
    assert_outcome("nothing_in_scope", "no_series", 0, 0, "inconclusive");
}

#[test]
fn shallow_unresolved_and_failed_analysis_never_emit_outputs() {
    for stage in 0..4 {
        let host = FakeHost::new(&analysis("analyze-history"));
        for response in ["false", host.root.join("checkout").to_str().unwrap(), SHA]
            .into_iter()
            .take(stage)
        {
            host.reply(response);
        }
        host.fail();
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
        assert!(host.outputs.borrow().is_empty());
    }
    for shallow in ["true", "not-a-boolean"] {
        let host = FakeHost::new(&analysis("analyze-history"));
        host.reply(shallow);
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
        assert_eq!(host.processes.borrow().len(), 1);
        assert!(host.scratches.borrow().is_empty());
    }
}

fn reject_report_field(field: &str, value: Value) {
    let mut host = FakeHost::new(&analysis("analyze-history"));
    host.analysis_replies();
    host.reports("history", "clean", "full", 1, 1);
    let path = host.root.join("temp").join("owned").join("report.json");
    let mut report: Value = serde_json::from_slice(host.files.get(&path).unwrap()).unwrap();
    report[field] = value;
    host.files
        .insert(path, serde_json::to_vec(&report).unwrap());
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn wrong_report_mode_is_rejected() {
    reject_report_field("mode", json!("branch"));
}

#[test]
fn wrong_report_commit_is_rejected() {
    reject_report_field("tip_commit", json!("b".repeat(40)));
}

#[test]
fn dirty_report_is_rejected() {
    reject_report_field("tip_dirty", json!(true));
}

#[test]
fn unknown_report_outcome_is_rejected() {
    reject_report_field("outcome", json!("unknown"));
}

#[test]
fn missing_regression_count_is_rejected() {
    reject_report_field("regressions", json!(null));
}

fn reject_blank_artifact(file: &str) {
    let mut host = FakeHost::new(&analysis("analyze-pr"));
    host.analysis_replies();
    host.reports("branch", "clean", "full", 1, 1);
    host.files.insert(
        host.root.join("temp").join("owned").join(file),
        b" ".to_vec(),
    );
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn blank_outcome_file_is_rejected() {
    reject_blank_artifact("outcome.txt");
}

#[test]
fn blank_summary_is_rejected() {
    reject_blank_artifact("summary.md");
}

#[test]
fn blank_markdown_report_is_rejected() {
    reject_blank_artifact("report.md");
}

#[test]
fn blank_json_report_is_rejected() {
    reject_blank_artifact("report.json");
}

#[test]
fn analysis_rejects_missing_keys_and_checkout_artifacts_before_main_process() {
    for location in ["keys", "temp", "cache"] {
        let mut input = analysis("analyze-history");
        if location == "cache" {
            input["cache"] = json!("cache");
        }
        let mut host = FakeHost::new(&input);
        host.analysis_replies();
        if location == "keys" {
            host.keys.clear();
        }
        let mut args = host.args();
        if location == "temp" {
            args.temp_dir = "temp".into();
        }
        block_on(run_with(args, &host, &FakePublisher::default())).unwrap_err();
        assert!(
            host.processes
                .borrow()
                .iter()
                .all(|process| process.program == "git")
        );
        assert!(host.outputs.borrow().is_empty());
        assert!(host.scratches.borrow().is_empty());
    }
}

#[test]
fn rejected_inputs_do_no_benchmark_or_publication_work() {
    let host = FakeHost::new(&json!({"command":"collect", "since":"30d"}));
    let publisher = FakePublisher::default();
    block_on(run_with(host.args(), &host, &publisher)).unwrap_err();
    assert!(host.processes.borrow().is_empty());
    assert!(publisher.commands.borrow().is_empty());
    assert!(host.environment_reads.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn non_utf8_report_content_is_not_a_successful_artifact() {
    let mut host = FakeHost::new(&analysis("analyze-history"));
    host.analysis_replies();
    host.reports("history", "clean", "full", 1, 1);
    host.files.insert(
        host.root.join("temp").join("owned").join("summary.md"),
        vec![255],
    );
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
}
