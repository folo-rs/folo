use futures::executor::block_on;
use serde_json::{Value, json};

use crate::action::execute::run_with;
use crate::action::tests::fake::{FakeHost, FakePublisher, SHA, report_input};
use crate::cli::{Command, Conclusion};

fn event(source: &str) -> Value {
    json!({"repository":{"full_name":"owner/repo"}, "number":7,
    "pull_request":{
        "head":{"sha":SHA, "repo":{"full_name":source}},
        "base":{"sha":"b".repeat(40), "repo":{"full_name":"owner/repo"}}
    }})
}

#[test]
fn fork_and_target_events_skip_before_processes_publication_or_credential_reads() {
    for name in ["pull_request", "pull_request_target"] {
        for command in ["collect", "alert", "publish-comment-preflight"] {
            let mut input = json!({"command":command});
            if command == "publish-comment-preflight" {
                input["packages"] = json!("crate");
            }
            let mut host = FakeHost::new(&input);
            host.event(name, &event("fork/repo"));
            let publisher = FakePublisher::default();
            block_on(run_with(host.args(), &host, &publisher)).unwrap();
            assert!(host.processes.borrow().is_empty());
            assert!(publisher.commands.borrow().is_empty());
            assert_eq!(host.notes.borrow().len(), 1);
            let outputs = host.outputs.borrow().first().unwrap().1.clone();
            assert!(outputs.contains("skipped=true\n"));
            assert!(outputs.contains("instance=action-test\n"));
        }
    }
}

#[test]
fn pr_event_identity_is_required_and_deleted_sources_are_skipped() {
    let mut host = FakeHost::new(&json!({"command":"collect"}));
    host.env.insert(
        "GITHUB_EVENT_NAME".to_owned(),
        "pull_request_target".to_owned(),
    );
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert!(host.processes.borrow().is_empty());
    let mut event = event("owner/repo");
    event["pull_request"]["head"]["repo"] = json!(null);
    host.event("pull_request", &event);
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
    assert!(
        host.outputs
            .borrow()
            .first()
            .unwrap()
            .1
            .contains("skipped=true\n")
    );
}

#[test]
fn publication_uses_event_real_head_not_merge_sha_and_legitimate_run_fallbacks() {
    let mut host =
        FakeHost::new(&json!({"command":"publish-comment-failed", "conclusion":"cancelled"}));
    host.event("pull_request_target", &event("OWNER/repo"));
    host.env.extend([
        ("GITHUB_RUN_ID".to_owned(), "42".to_owned()),
        ("GITHUB_RUN_ATTEMPT".to_owned(), "3".to_owned()),
        (
            "GITHUB_SERVER_URL".to_owned(),
            "https://github.com".to_owned(),
        ),
        ("GITHUB_SHA".to_owned(), "c".repeat(40)),
    ]);
    let publisher = FakePublisher::default();
    block_on(run_with(host.args(), &host, &publisher)).unwrap();
    let commands = publisher.commands.borrow();
    let (command, context) = commands.first().unwrap();
    let Command::PublishCommentFailed {
        pull_request,
        failed,
    } = command
    else {
        panic!()
    };
    assert_eq!(pull_request.get(), 7);
    assert_eq!(failed.pending.head.as_str(), SHA);
    assert_eq!(failed.pending.run.run_id.get(), 42);
    assert_eq!(failed.pending.run.run_attempt.get(), 3);
    assert_eq!(
        failed.run_url,
        "https://github.com/owner/repo/actions/runs/42"
    );
    assert_eq!(failed.conclusion, Conclusion::Cancelled);
    assert_eq!(context.repository.to_string(), "owner/repo");
    assert!(context.verbose);
    assert!(host.processes.borrow().is_empty());
}

#[test]
fn explicit_publication_fields_win_and_paths_use_measured_directory() {
    let mut input = report_input("publish-comment-findings");
    input["working-directory"] = json!("checkout");
    input["packages"] = json!("crate-a");
    input["pr-number"] = json!("99");
    input["artifact-url"] = json!("https://github.com/owner/repo/actions/runs/42/artifacts/7");
    let mut host = FakeHost::new(&input);
    host.env
        .insert("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned());
    host.env
        .insert("GITHUB_RUN_ID".to_owned(), "111".to_owned());
    let publisher = FakePublisher::default();
    block_on(run_with(host.args(), &host, &publisher)).unwrap();
    let commands = publisher.commands.borrow();
    let (command, context) = commands.first().unwrap();
    let Command::PublishCommentFindings(args) = command else {
        panic!()
    };
    assert_eq!(args.pull_request.get(), 99);
    assert_eq!(args.report.run.run_id.get(), 42);
    assert_eq!(
        args.report.body_file,
        host.root.join("checkout").join("summary.md")
    );
    assert_eq!(
        args.report.evidence.report_file,
        host.root.join("checkout").join("report.json")
    );
    assert_eq!(args.report.analyzed_sha.as_str(), SHA);
    assert_eq!(context.instance.as_str(), "checkout");
}

#[test]
fn no_execution_ids_or_conclusions_are_invented() {
    for missing in [
        "GITHUB_REPOSITORY",
        "GITHUB_RUN_ID",
        "GITHUB_RUN_ATTEMPT",
        "GITHUB_SHA",
        "GITHUB_SERVER_URL",
    ] {
        let mut host =
            FakeHost::new(&json!({"command":"publish-issue-failed", "conclusion":"failure"}));
        host.env.extend([
            ("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned()),
            ("GITHUB_RUN_ID".to_owned(), "42".to_owned()),
            ("GITHUB_RUN_ATTEMPT".to_owned(), "2".to_owned()),
            ("GITHUB_SHA".to_owned(), SHA.to_owned()),
            (
                "GITHUB_SERVER_URL".to_owned(),
                "https://github.com".to_owned(),
            ),
        ]);
        host.env.remove(missing).unwrap();
        let publisher = FakePublisher::default();
        block_on(run_with(host.args(), &host, &publisher)).unwrap_err();
        assert!(publisher.commands.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
    let mut host = FakeHost::new(
        &json!({"command":"publish-comment-preflight", "packages":"a", "run-id":"42", "run-attempt":"1", "head":SHA}),
    );
    host.env
        .insert("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned());
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
}

fn lifecycle_command(sink: &str, state: &str) -> Command {
    let command = format!("publish-{sink}-{state}");
    let mut input = if matches!(state, "findings" | "clean" | "no-data") {
        report_input(&command)
    } else {
        json!({"command":command, "head":SHA, "run-id":"42", "run-attempt":"2"})
    };
    if state == "failed" {
        input["conclusion"] = json!("failure");
        input["run-url"] = json!("https://github.com/owner/repo/actions/runs/42");
    }
    if sink == "comment" {
        input["pr-number"] = json!("7");
        if state != "failed" {
            input["packages"] = json!("crate");
        }
    }
    dispatch_input(&input)
}

fn dispatch_input(input: &Value) -> Command {
    let mut host = FakeHost::new(input);
    host.env
        .insert("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned());
    let publisher = FakePublisher::default();
    block_on(run_with(host.args(), &host, &publisher)).unwrap();
    assert_eq!(publisher.commands.borrow().len(), 1);
    assert_eq!(host.outputs.borrow().len(), 1);
    publisher.commands.into_inner().pop().unwrap().0
}

#[test]
fn comment_findings_dispatch() {
    assert!(matches!(
        lifecycle_command("comment", "findings"),
        Command::PublishCommentFindings(_)
    ));
}

#[test]
fn comment_clean_dispatch() {
    assert!(matches!(
        lifecycle_command("comment", "clean"),
        Command::PublishCommentClean(_)
    ));
}

#[test]
fn comment_preflight_dispatch() {
    assert!(matches!(
        lifecycle_command("comment", "preflight"),
        Command::PublishCommentPreflight { .. }
    ));
}

#[test]
fn comment_no_data_dispatch() {
    assert!(matches!(
        lifecycle_command("comment", "no-data"),
        Command::PublishCommentNoData { .. }
    ));
}

#[test]
fn comment_failed_dispatch() {
    assert!(matches!(
        lifecycle_command("comment", "failed"),
        Command::PublishCommentFailed { .. }
    ));
}

#[test]
fn issue_findings_dispatch() {
    assert!(matches!(
        lifecycle_command("issue", "findings"),
        Command::PublishIssueFindings(_)
    ));
}

#[test]
fn issue_clean_dispatch() {
    assert!(matches!(
        lifecycle_command("issue", "clean"),
        Command::PublishIssueClean(_)
    ));
}

#[test]
fn issue_preflight_dispatch() {
    assert!(matches!(
        lifecycle_command("issue", "preflight"),
        Command::PublishIssuePreflight(_)
    ));
}

#[test]
fn issue_no_data_dispatch() {
    assert!(matches!(
        lifecycle_command("issue", "no-data"),
        Command::PublishIssueNoData(_)
    ));
}

#[test]
fn issue_failed_dispatch() {
    assert!(matches!(
        lifecycle_command("issue", "failed"),
        Command::PublishIssueFailed(_)
    ));
}

#[test]
fn empty_comment_scope_needs_no_report() {
    let command = dispatch_input(&json!({"command":"publish-comment-no-data",
        "run-id":"42", "run-attempt":"2", "empty-scope":"true", "head":SHA, "pr-number":"7"}));
    let Command::PublishCommentNoData { data, packages, .. } = command else {
        panic!()
    };
    assert!(data.empty_scope);
    assert!(data.report_file.is_none());
    assert!(packages.is_none());
}

#[test]
fn empty_issue_scope_needs_no_report() {
    let command = dispatch_input(&json!({"command":"publish-issue-no-data",
        "run-id":"42", "run-attempt":"2", "empty-scope":"true", "head":SHA}));
    let Command::PublishIssueNoData(data) = command else {
        panic!()
    };
    assert!(data.empty_scope);
    assert!(data.report_file.is_none());
}

#[test]
fn alert_has_no_report_or_attempt_fabrication() {
    let command = dispatch_input(&json!({"command":"alert", "run-id":"42",
        "run-url":"https://github.com/owner/repo/actions/runs/42"}));
    let Command::Alert { run_id, run_url } = command else {
        panic!()
    };
    assert_eq!(run_id.get(), 42);
    assert_eq!(run_url, "https://github.com/owner/repo/actions/runs/42");
}

#[test]
fn publication_failure_withholds_success_outputs() {
    let mut host = FakeHost::new(
        &json!({"command":"alert", "run-id":"42", "run-url":"https://github.com/owner/repo/actions/runs/42"}),
    );
    host.env
        .insert("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned());
    let publisher = FakePublisher {
        fail: true,
        ..FakePublisher::default()
    };
    block_on(run_with(host.args(), &host, &publisher)).unwrap_err();
    assert_eq!(publisher.commands.borrow().len(), 1);
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn malformed_events_and_invalid_environment_identity_are_rejected() {
    let mut host = FakeHost::new(&json!({"command":"collect"}));
    host.env
        .insert("GITHUB_EVENT_PATH".to_owned(), "event.json".to_owned());
    host.files
        .insert(host.root.join("event.json"), b"not json".to_vec());
    block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert!(host.processes.borrow().is_empty());

    for (key, value) in [
        ("GITHUB_RUN_ID", "0"),
        ("GITHUB_RUN_ATTEMPT", "0"),
        ("GITHUB_SHA", "not-a-sha"),
    ] {
        let mut host = FakeHost::new(&json!({"command":"publish-issue-preflight"}));
        host.env.extend([
            ("GITHUB_REPOSITORY".to_owned(), "owner/repo".to_owned()),
            ("GITHUB_RUN_ID".to_owned(), "42".to_owned()),
            ("GITHUB_RUN_ATTEMPT".to_owned(), "2".to_owned()),
            ("GITHUB_SHA".to_owned(), SHA.to_owned()),
        ]);
        host.env.insert(key.to_owned(), value.to_owned());
        let publisher = FakePublisher::default();
        block_on(run_with(host.args(), &host, &publisher)).unwrap_err();
        assert!(publisher.commands.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}
