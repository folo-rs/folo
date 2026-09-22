#![allow(
    clippy::indexing_slicing,
    reason = "These tests mutate known JSON fixtures to exercise individual validation paths."
)]

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::time::{Duration, SystemTime};

use futures::executor::block_on;
use jiff::Timestamp;
use ohno::AppError;
use serde_json::{Value, json};
use tick::Clock;

use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::port::Output;
use crate::action::preparation::inputs::{WorkflowInputs, package_name};
use crate::action::preparation::scope::Workspace;
use crate::action::preparation::{Flow, PrepareWorkflowArgs, prepare_with as prepare_with_clock};
use crate::action::tests::fake::{FakeHost, SHA};

const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RANGE_END: &str = "cccccccccccccccccccccccccccccccccccccccc";

// A month-end in a leap year distinguishes UTC calendar subtraction from fixed-day arithmetic.
const NOW: &str = "2024-03-31T12:00:00Z";

fn clock(now: &str) -> Clock {
    Clock::new_frozen_at(SystemTime::from(now.parse::<Timestamp>().unwrap()))
}

fn age_argument(name: &str, value: &str) -> OsString {
    format!(
        "--{name}={}",
        value.parse::<Timestamp>().unwrap().as_second()
    )
    .into()
}

async fn prepare_with(args: PrepareWorkflowArgs, host: &FakeHost) -> Result<(), AppError> {
    prepare_with_clock(args, host, &clock(NOW)).await
}

fn args(host: &FakeHost, flow: Flow) -> PrepareWorkflowArgs {
    PrepareWorkflowArgs {
        flow,
        inputs_file: host.root.join("inputs.json"),
        github_output: "outputs".into(),
    }
}

fn event() -> Value {
    json!({"number":7, "pull_request":{
        "head":{"sha":SHA, "repo":{"full_name":"owner/repo"}},
        "base":{"sha":BASE, "repo":{"full_name":"owner/repo"}}
    }})
}

fn metadata(host: &FakeHost) -> Value {
    let mut packages = Vec::new();
    for (name, kind, dependency) in [
        ("library", "lib", None),
        ("middle", "lib", Some("library")),
        ("benchmark", "bench", Some("middle")),
        ("unrelated", "bench", None),
    ] {
        let dependencies = dependency.map_or_else(Vec::new, |name| {
            vec![json!({"path":host.root.join(name), "kind":"dev"})]
        });
        packages.push(json!({
            "id":name, "name":name, "manifest_path":host.root.join(name).join("Cargo.toml"),
            "targets":[{"kind":[kind]}], "dependencies":dependencies,
        }));
    }
    json!({
        "workspace_root":host.root,
        "workspace_members":["library","middle","benchmark","unrelated"],
        "packages":packages,
    })
}

fn prepare_responses(host: &FakeHost, flow: Flow) {
    host.reply("false");
    host.reply(SHA);
    if flow == Flow::Pr {
        host.reply(BASE);
    }
    host.reply(&metadata(host).to_string());
}

// Boundary cases need one candidate, not the separate dependency-closure fixture.
fn prepare_single_member_responses(host: &FakeHost) {
    host.reply("false");
    host.reply(SHA);
    host.reply(BASE);
    host.reply(
        &json!({
            "workspace_root":host.root,
            "workspace_members":["owner"],
            "packages":[{
                "id":"owner", "name":"owner",
                "manifest_path":host.root.join("owner").join("Cargo.toml"),
                "targets":[{"kind":["bench"]}], "dependencies":[],
            }],
        })
        .to_string(),
    );
}

fn output(host: &FakeHost) -> String {
    let outputs = host.outputs.borrow();
    assert_eq!(outputs.len(), 1);
    outputs[0].1.clone()
}

#[test]
fn workflow_inputs_reject_non_strings_duplicates_unknown_fields_and_invalid_scope() {
    for json in [
        "[]",
        r#"{"platforms":true}"#,
        r#"{"platforms":"linux","platforms":"windows"}"#,
        r#"{"platforms":"linux","command":"collect"}"#,
        r#"{"platforms":"linux","scope":"unknown"}"#,
        r#"{"platforms":"linux","scope":"workspace"}"#,
        r#"{"platforms":"linux","scope":"affected"}"#,
        r#"{"platforms":"linux,windows,"}"#,
        r#"{"platforms":"linux","exclude":"member,"}"#,
        r#"{"platforms":"linux","working-directory":" "}"#,
        r#"{"platforms":"linux","config":"line\nbreak"}"#,
    ] {
        WorkflowInputs::parse(json.as_bytes(), Flow::History)
            .err()
            .unwrap();
    }
    let inputs = WorkflowInputs::parse(
        br#"{"platforms":"linux","config":"","exclude":" library,library "}"#,
        Flow::History,
    )
    .unwrap();
    assert_eq!(inputs.excluded, BTreeSet::from(["library".to_owned()]));
}

#[test]
fn backfill_range_inputs_are_required_and_specific_to_the_backfill_flow() {
    for input in [
        json!({"platforms":"linux", "to":"HEAD"}),
        json!({"platforms":"linux", "from":"HEAD"}),
        json!({"platforms":"linux", "from":"", "to":"HEAD"}),
        json!({"platforms":"linux", "from":"HEAD", "to":""}),
        json!({"platforms":"linux", "from":"--option", "to":"HEAD"}),
        json!({"platforms":"linux", "from":"HEAD", "to":"--option"}),
        json!({"platforms":"linux", "from":true, "to":"HEAD"}),
    ] {
        let error = WorkflowInputs::parse(&serde_json::to_vec(&input).unwrap(), Flow::Backfill)
            .err()
            .unwrap();
        assert!(error.find_source::<InvalidInput>().is_some());
    }
    for flow in [Flow::History, Flow::Pr] {
        let error = WorkflowInputs::parse(br#"{"platforms":"linux","from":"","to":""}"#, flow)
            .err()
            .unwrap();
        assert!(error.find_source::<InvalidInput>().is_some());
    }
}

#[test]
fn invalid_backfill_inputs_stop_preparation_before_processes_or_outputs() {
    let host = FakeHost::new(&json!({"platforms":"linux", "to":"HEAD"}));
    let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
    assert!(error.find_source::<InvalidInput>().is_some());
    assert!(host.processes.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn adapter_empty_range_defaults_select_exact_mode() {
    let host = FakeHost::new(&json!({
        "working-directory":"", "config":"", "platforms":"linux", "exclude":"",
        "from":"HEAD~1", "to":"HEAD", "lookback":"", "minimum-age":"",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(BASE);
    host.reply(RANGE_END);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    assert!(output(&host).contains(&format!("from={BASE}\nto={RANGE_END}\n")));
    assert!(output(&host).contains("has-work=true\n"));
    let processes = host.processes.borrow();
    assert_eq!(processes.len(), 4);
    assert_eq!(processes[2].args.last().unwrap(), "HEAD~1^{commit}");
    assert_eq!(processes[3].args.last().unwrap(), "HEAD^{commit}");
}

#[test]
fn adapter_empty_range_defaults_select_rolling_mode_and_allow_override() {
    for to in ["", "release"] {
        let host = FakeHost::new(&json!({
            "working-directory":"", "config":"", "platforms":"linux", "exclude":"",
            "from":"", "to":to, "lookback":"14 days", "minimum-age":"24 hours",
        }));
        host.reply("false");
        host.reply(SHA);
        host.reply(RANGE_END);
        host.reply(BASE);
        block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
        assert!(output(&host).contains(&format!("from={BASE}\nto={RANGE_END}\n")));
        assert!(output(&host).contains("has-work=true\n"));
        let processes = host.processes.borrow();
        assert_eq!(processes.len(), 4);
        if to.is_empty() {
            assert!(processes[2].args.contains(&"--max-count=1".into()));
        } else {
            assert_eq!(processes[2].args.last().unwrap(), "release^{commit}");
        }
    }
}

#[test]
fn adapter_defaults_do_not_hide_unknown_or_whitespace_inputs() {
    for (key, value) in [("unknown", ""), ("from", " "), ("to", "\t")] {
        let mut input = json!({
            "platforms":"linux", "from":"", "to":"",
            "lookback":"14 days", "minimum-age":"24 hours",
        });
        input[key] = json!(value);
        let host = FakeHost::new(&input);
        let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
        assert_eq!(error.find_source::<InvalidInput>().unwrap().input, key);
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_backfill_requires_both_durations_and_rejects_exact_mode_keys() {
    for input in [
        json!({"lookback":"14 days"}),
        json!({"minimum-age":"1 day"}),
        json!({"from":"HEAD~1", "to":"HEAD", "lookback":"14 days", "minimum-age":"1 day"}),
        json!({"from":"HEAD~1", "to":"HEAD", "minimum-age":"0 seconds"}),
    ] {
        let mut input = input;
        input["platforms"] = json!("linux");
        let host = FakeHost::new(&input);
        let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
        assert!(error.find_source::<InvalidInput>().is_some());
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_backfill_rejects_zero_lookback_and_absolute_dates() {
    for (key, value) in [
        ("lookback", "0 seconds"),
        ("lookback", "2024-03-01"),
        ("minimum-age", "2024-03-01T12:00:00Z"),
        ("minimum-age", "invalid"),
    ] {
        let mut input = json!({"platforms":"linux", "lookback":"14 days", "minimum-age":"1 day"});
        input[key] = json!(value);
        let host = FakeHost::new(&input);
        let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
        assert_eq!(error.find_source::<InvalidInput>().unwrap().input, key);
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_backfill_rejects_blank_durations_and_invalid_overrides() {
    for (key, value) in [
        ("lookback", ""),
        ("minimum-age", " "),
        ("to", " "),
        ("to", "--option"),
    ] {
        let mut input = json!({"platforms":"linux", "lookback":"14 days", "minimum-age":"1 day"});
        input[key] = json!(value);
        let host = FakeHost::new(&input);
        let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
        assert_eq!(error.find_source::<InvalidInput>().unwrap().input, key);
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_inputs_are_not_history_or_pr_configuration() {
    for flow in [Flow::History, Flow::Pr] {
        let host = FakeHost::new(&json!({
            "platforms":"linux", "lookback":"14 days", "minimum-age":"1 day",
        }));
        let error = block_on(prepare_with(args(&host, flow), &host)).unwrap_err();
        assert!(error.find_source::<InvalidInput>().is_some());
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_backfill_freezes_now_relative_cutoffs_and_oldest_first_parent() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"24 hours",
        "exclude":"historical-only",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(RANGE_END);
    host.reply(&format!("{RANGE_END}\n{BASE}\n"));
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    let outputs = output(&host);
    assert!(outputs.contains(&format!("from={BASE}\nto={RANGE_END}\n")));
    assert!(outputs.contains("skipped=false\nhas-work=true\n"));
    assert!(!outputs.contains("no-work-reason="));
    let processes = host.processes.borrow();
    assert_eq!(processes.len(), 4);
    assert_eq!(
        processes[2].args,
        [
            "rev-list".into(),
            "--first-parent".into(),
            "--max-count=1".into(),
            age_argument("min-age", "2024-03-30T12:00:00Z"),
            SHA.into(),
            "--".into(),
        ]
    );
    assert_eq!(
        processes[3].args,
        [
            "rev-list".into(),
            "--first-parent".into(),
            age_argument("max-age", "2024-03-17T12:00:00Z"),
            RANGE_END.into(),
            "--".into(),
        ]
    );
    assert!(host.package_queries.borrow().is_empty());
}

#[test]
fn rolling_subsecond_window_rounds_the_lower_bound_up_and_upper_bound_down() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"1ns", "minimum-age":"0 seconds",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(RANGE_END);
    host.reply("");
    block_on(prepare_with_clock(
        args(&host, Flow::Backfill),
        &host,
        &clock("2024-03-31T12:00:00.000000100Z"),
    ))
    .unwrap();
    let processes = host.processes.borrow();
    assert!(processes[2].args.contains(&age_argument("min-age", NOW)));
    assert!(
        processes[3]
            .args
            .contains(&age_argument("max-age", "2024-03-31T12:00:01Z"))
    );
    assert!(output(&host).contains(&format!("from={RANGE_END}\nto={RANGE_END}\n")));
}

#[test]
fn rolling_backfill_uses_span_magnitudes_and_utc_calendar_months() {
    for lookback in ["1 month ago", "-P1M"] {
        let host = FakeHost::new(&json!({
            "platforms":"linux", "lookback":lookback, "minimum-age":"PT0S",
        }));
        host.reply("false");
        host.reply(SHA);
        host.reply(RANGE_END);
        host.reply(BASE);
        block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
        let processes = host.processes.borrow();
        assert!(processes[2].args.contains(&age_argument("min-age", NOW)));
        assert!(
            processes[3]
                .args
                .contains(&age_argument("max-age", "2024-02-29T12:00:00Z"))
        );
        assert!(output(&host).contains(&format!("from={BASE}\nto={RANGE_END}\n")));
    }
}

#[test]
fn rolling_override_bypasses_age_selection_but_not_the_now_relative_horizon() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"1 month ago", "to":"release",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(RANGE_END);
    host.reply(BASE);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    let processes = host.processes.borrow();
    assert_eq!(processes.len(), 4);
    assert!(processes[2].args.contains(&"--end-of-options".into()));
    assert_eq!(processes[2].args.last().unwrap(), "release^{commit}");
    assert!(
        processes[3]
            .args
            .contains(&age_argument("max-age", "2024-03-17T12:00:00Z"))
    );
    assert!(
        processes
            .iter()
            .flat_map(|process| &process.args)
            .all(|arg| !arg.to_str().unwrap().starts_with("--min-age="))
    );
    assert!(output(&host).contains(&format!("from={BASE}\nto={RANGE_END}\n")));
}

#[test]
fn rolling_backfill_distinguishes_no_eligible_endpoint_from_a_fork_skip() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"1 day",
        "from":"", "to":"",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply("");
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    let outputs = output(&host);
    assert!(outputs.contains("skipped=false\nhas-work=false\nno-work-reason=no-eligible-commit\n"));
    for field in ["from=", "to=", "skip-reason=", "skip-all=", "packages="] {
        assert!(!outputs.lines().any(|line| line.starts_with(field)));
    }
    assert_eq!(host.processes.borrow().len(), 3);
}

#[test]
fn rolling_backfill_keeps_a_single_endpoint_outside_the_lookback_window() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"1 day", "minimum-age":"1 month",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(BASE);
    host.reply("");
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    assert!(output(&host).contains(&format!("from={BASE}\nto={BASE}\n")));
    assert!(output(&host).contains("has-work=true\n"));
}

#[test]
fn rolling_calendar_cutoffs_before_git_epoch_keep_their_meaning() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"100 years", "minimum-age":"1 day",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(RANGE_END);
    host.reply(BASE);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    assert!(
        host.processes.borrow()[3]
            .args
            .contains(&"--max-age=0".into())
    );
    assert!(output(&host).contains(&format!("from={BASE}\nto={RANGE_END}\n")));

    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"100 years", "minimum-age":"100 years",
    }));
    host.reply("false");
    host.reply(SHA);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    assert!(output(&host).contains("has-work=false\nno-work-reason=no-eligible-commit\n"));
    assert_eq!(host.processes.borrow().len(), 2);
}

#[test]
fn rolling_cutoff_at_git_epoch_still_queries_for_an_eligible_commit() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"1 second", "minimum-age":"0 seconds",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(BASE);
    host.reply(BASE);
    block_on(prepare_with_clock(
        args(&host, Flow::Backfill),
        &host,
        &clock("1970-01-01T00:00:00Z"),
    ))
    .unwrap();
    assert_eq!(host.processes.borrow().len(), 4);
    assert!(
        host.processes.borrow()[2]
            .args
            .contains(&"--min-age=0".into())
    );
    assert!(output(&host).contains(&format!("from={BASE}\nto={BASE}\n")));
}

#[test]
fn rolling_clock_outside_calendar_range_withholds_outputs() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"1 day",
    }));
    host.reply("false");
    host.reply(SHA);
    // Advance just beyond Jiff's representable timestamps without reading a real clock.
    let time = SystemTime::from(Timestamp::MAX)
        .checked_add(Duration::from_secs(1))
        .unwrap();
    let error = block_on(prepare_with_clock(
        args(&host, Flow::Backfill),
        &host,
        &Clock::new_frozen_at(time),
    ))
    .unwrap_err();
    assert!(error.find_source::<InvalidOutput>().is_some());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn rolling_calendar_overflow_withholds_outputs_even_with_an_override() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"15000 years", "to":"release",
    }));
    host.reply("false");
    host.reply(SHA);
    let error = block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
    assert_eq!(
        error.find_source::<InvalidInput>().unwrap().input,
        "minimum-age"
    );
    assert!(host.outputs.borrow().is_empty());
    assert_eq!(host.processes.borrow().len(), 2);
}

#[test]
fn rolling_git_errors_do_not_become_no_work_results() {
    for response in [None, Some("invalid")] {
        let host = FakeHost::new(&json!({
            "platforms":"linux", "lookback":"14 days", "minimum-age":"1 day",
        }));
        host.reply("false");
        host.reply(SHA);
        if let Some(response) = response {
            host.reply(response);
        } else {
            host.fail();
        }
        block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn rolling_history_validates_every_commit_instead_of_only_the_oldest() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "lookback":"14 days", "minimum-age":"1 day",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply(RANGE_END);
    host.reply(&format!("invalid\n{BASE}\n"));
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn backfill_freezes_refs_without_filtering_historical_scope_through_head_metadata() {
    let mut host = FakeHost::new(&json!({
        "platforms":"windows,linux", "config":"authority.toml",
        "from":"release~2", "to":"release", "exclude":"historical-only",
    }));
    host.files.insert(
        host.root.join("authority.toml"),
        b"[project]\nid='Historical Project'".to_vec(),
    );
    host.reply("false");
    host.reply(SHA);
    host.reply(BASE);
    host.reply(RANGE_END);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    let outputs = output(&host);
    assert!(outputs.contains("instance=historical_project\n"));
    assert!(outputs.contains("matrix={\"platform\":[\"linux\",\"windows\"]}\n"));
    assert!(outputs.contains(&format!("from={BASE}\nto={RANGE_END}\nskipped=false\n")));
    assert!(outputs.contains("has-work=true\n"));
    for field in [
        "head=",
        "base=",
        "packages=",
        "skip-all=",
        "collection-job-prefix=",
    ] {
        assert!(!outputs.lines().any(|line| line.starts_with(field)));
    }
    let processes = host.processes.borrow();
    assert_eq!(processes.len(), 4);
    assert!(processes.iter().all(|process| process.program == "git"));
    let from = processes.get(2).unwrap();
    assert!(from.args.contains(&"--end-of-options".into()));
    assert_eq!(from.args.last().unwrap(), "release~2^{commit}");
    let to = processes.get(3).unwrap();
    assert!(to.args.contains(&"--end-of-options".into()));
    assert_eq!(to.args.last().unwrap(), "release^{commit}");
    assert!(host.package_queries.borrow().is_empty());
}

#[test]
fn backfill_fork_skip_has_no_range_or_empty_collection_scope() {
    let mut host = FakeHost::new(&json!({
        "platforms":"linux", "from":"HEAD~1", "to":"HEAD",
    }));
    let mut event = event();
    event["pull_request"]["head"]["repo"]["full_name"] = json!("fork/repo");
    host.event("pull_request", &event);
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap();
    let outputs = output(&host);
    assert!(outputs.contains("skipped=true\nskip-reason=fork-pull-request\n"));
    assert!(outputs.contains("has-work=false\n"));
    for field in ["from=", "to=", "packages=", "skip-all="] {
        assert!(!outputs.lines().any(|line| line.starts_with(field)));
    }
    assert!(host.processes.borrow().is_empty());
}

#[test]
fn an_unresolved_backfill_endpoint_withholds_all_outputs() {
    let host = FakeHost::new(&json!({
        "platforms":"linux", "from":"missing", "to":"HEAD",
    }));
    host.reply("false");
    host.reply(SHA);
    host.reply("not-a-commit");
    block_on(prepare_with(args(&host, Flow::Backfill), &host)).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
    assert_eq!(host.processes.borrow().len(), 3);
}

#[test]
fn history_workspace_scope_and_matrix_share_canonical_namespace() {
    let mut host = FakeHost::new(&json!({
        "platforms":" windows,linux,linux ", "config":"authority.toml",
    }));
    host.files.insert(
        host.root.join("authority.toml"),
        b"[project]\nid='Custom Project!'".to_vec(),
    );
    prepare_responses(&host, Flow::History);
    block_on(prepare_with(args(&host, Flow::History), &host)).unwrap();
    let output = output(&host);
    assert!(output.contains("instance=custom_project_\n"));
    assert!(output.contains("matrix={\"platform\":[\"linux\",\"windows\"]}\n"));
    assert!(output.contains("expected-platforms=linux,windows\n"));
    assert!(output.contains("collection-job-prefix=cbh-collect:custom_project_\n"));
    assert!(output.contains(&format!("head={SHA}\nbase={SHA}\n")));
    assert!(output.contains("packages=benchmark,unrelated\nskip-all=false\nskipped=false\n"));
    assert!(host.package_queries.borrow().is_empty());
    assert_eq!(host.processes.borrow().len(), 3);
    assert!(
        host.processes
            .borrow()
            .iter()
            .all(|process| process.output == Output::Capture)
    );
    assert!(
        !host
            .environment_reads
            .borrow()
            .iter()
            .any(|name| matches!(name.as_str(), "GITHUB_TOKEN" | "GH_TOKEN"))
    );
}

#[test]
fn history_uses_the_checked_checkout_head_as_its_base() {
    let host = FakeHost::new(&json!({"platforms":"linux", "exclude":"unrelated"}));
    prepare_responses(&host, Flow::History);
    block_on(prepare_with(args(&host, Flow::History), &host)).unwrap();
    assert!(output(&host).contains(&format!("head={SHA}\nbase={SHA}\npackages=benchmark\n")));
    let processes = host.processes.borrow();
    let cargo = processes.last().unwrap();
    assert_eq!(cargo.program, "cargo");
    assert!(cargo.args.contains(&"--offline".into()));
    assert!(cargo.args.contains(&"--locked".into()));
}

#[test]
fn history_on_a_pull_request_uses_real_head_instead_of_merge_sha() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    host.env.insert("GITHUB_SHA".to_owned(), "c".repeat(40));
    prepare_responses(&host, Flow::History);
    block_on(prepare_with(args(&host, Flow::History), &host)).unwrap();
    assert!(output(&host).contains(&format!("head={SHA}\nbase={SHA}\n")));
    assert!(output(&host).contains("packages=benchmark,unrelated\n"));
    assert!(host.package_queries.borrow().is_empty());
}

#[test]
fn pr_preparation_rejects_other_event_kinds_before_starting_processes() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request_target", &event());
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
    assert!(host.processes.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn pr_preparation_requires_number_even_with_the_correct_event_kind() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    let mut event = event();
    event.as_object_mut().unwrap().remove("number").unwrap();
    host.event("pull_request", &event);
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
    assert!(host.processes.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn resolved_base_must_match_the_frozen_event_commit() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    host.reply("false");
    host.reply(SHA);
    host.reply(SHA);
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn affected_scope_expands_through_excluded_nonbenchmark_dependencies() {
    let mut host = FakeHost::new(&json!({
        "platforms":"linux", "exclude":"library,middle",
    }));
    host.event("pull_request", &event());
    host.env.insert("GITHUB_SHA".to_owned(), "c".repeat(40));
    prepare_responses(&host, Flow::Pr);
    host.reply(host.root.to_str().unwrap());
    host.reply("library/deleted.rs\0library/new.rs\0");
    host.packages
        .insert(host.root.join("library"), Some("library".to_owned()));
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains(&format!("head={SHA}\nbase={BASE}\n")));
    assert!(output(&host).contains("packages=benchmark\nskip-all=false\n"));
    assert_eq!(host.package_queries.borrow().len(), 2);
    let processes = host.processes.borrow();
    let diff = processes.last().unwrap();
    assert!(diff.args.contains(&"--no-renames".into()));
    assert!(diff.args.contains(&"-z".into()));
    assert!(diff.args.contains(&format!("{BASE}...{SHA}").into()));
}

#[test]
fn empty_affected_scope_is_explicit_instead_of_a_workspace_fallback() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    prepare_responses(&host, Flow::Pr);
    host.reply(host.root.to_str().unwrap());
    host.reply("");
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains("packages=\nskip-all=true\nskipped=false\n"));
    assert!(host.package_queries.borrow().is_empty());
}

#[test]
fn workspace_file_changes_select_every_benchmark_package() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    prepare_responses(&host, Flow::Pr);
    host.reply(host.root.to_str().unwrap());
    host.reply("Cargo.toml\0");
    host.packages.insert(host.root.join("Cargo.toml"), None);
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains("packages=benchmark,unrelated\n"));
}

#[test]
fn independent_fixture_workspaces_outside_members_select_the_workspace() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    prepare_single_member_responses(&host);
    host.reply(host.root.to_str().unwrap());
    host.reply(".github/fixture/Cargo.toml\0");
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains("packages=owner\n"));
    assert!(host.package_queries.borrow().is_empty());
}

#[test]
fn fixture_workspaces_within_members_query_the_declared_package_boundary() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    prepare_single_member_responses(&host);
    host.reply(host.root.to_str().unwrap());
    host.reply("owner/tests/fixture/Cargo.toml\0");
    host.packages
        .insert(host.root.join("owner"), Some("owner".to_owned()));
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains("packages=owner\n"));
    let queries = host.package_queries.borrow();
    assert_eq!(queries.len(), 1);
    assert_eq!(queries[0].1, host.root.join("owner"));
}

#[test]
fn declared_member_boundaries_choose_the_deepest_member_without_prefix_collisions() {
    let host = FakeHost::new(&json!({}));
    let mut metadata = metadata(&host);
    metadata["workspace_members"]
        .as_array_mut()
        .unwrap()
        .push(json!("nested"));
    let nested = host.root.join("library").join("nested");
    metadata["packages"].as_array_mut().unwrap().push(json!({
        "id":"nested", "name":"nested", "manifest_path":nested.join("Cargo.toml"),
        "targets":[{"kind":["bench"]}], "dependencies":[],
    }));
    let workspace = Workspace::parse(&metadata.to_string(), &host).unwrap();
    assert_eq!(
        workspace.package_directory(&nested.join("src").join("lib.rs")),
        Some(nested.as_path())
    );
    assert_eq!(
        workspace.package_directory(&host.root.join("library-other").join("lib.rs")),
        None
    );
}

#[test]
fn fork_skip_is_not_an_empty_scope_verdict_and_starts_no_process() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    let mut event = event();
    event["pull_request"]["head"]["repo"]["full_name"] = json!("fork/repo");
    host.event("pull_request", &event);
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap();
    assert!(output(&host).contains("skipped=true\nskip-reason=fork-pull-request\n"));
    assert!(!output(&host).contains("head="));
    assert!(host.processes.borrow().is_empty());
}

#[test]
fn checkout_mismatch_shallow_history_and_process_failures_withhold_outputs() {
    for failure in ["head", "shallow", "status", "process", "missing-event"] {
        let mut host = FakeHost::new(&json!({"platforms":"linux"}));
        if failure != "missing-event" {
            host.event("pull_request", &event());
        }
        match failure {
            "head" => {
                host.reply("false");
                host.reply(BASE);
            }
            "shallow" => host.reply("true"),
            "status" => host.reply("unexpected"),
            "process" => host.fail(),
            _ => {}
        }
        let error = block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
        if failure == "shallow" {
            assert_eq!(
                error.find_source::<InvalidInput>().unwrap().input,
                "checkout"
            );
        } else if failure == "status" {
            assert!(error.find_source::<InvalidOutput>().is_some());
        }
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn scope_closure_handles_cycles_and_ignores_registry_edges() {
    let host = FakeHost::new(&json!({}));
    let mut metadata = metadata(&host);
    metadata["packages"][1]["dependencies"]
        .as_array_mut()
        .unwrap()
        .push(json!({"path":host.root.join("benchmark")}));
    metadata["packages"][3]["dependencies"] = json!([{"name":"library","path":null}]);
    let workspace = Workspace::parse(&metadata.to_string(), &host).unwrap();
    assert_eq!(
        workspace
            .select(
                Some(BTreeSet::from(["library".to_owned()])),
                &BTreeSet::new()
            )
            .unwrap(),
        BTreeSet::from(["benchmark".to_owned()])
    );
    assert_eq!(
        workspace.select(None, &BTreeSet::new()).unwrap(),
        BTreeSet::from(["benchmark".to_owned(), "unrelated".to_owned()])
    );
    assert!(
        workspace
            .select(
                None,
                &BTreeSet::from(["benchmark".to_owned(), "unrelated".to_owned()])
            )
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_scope_members_and_incomplete_metadata_are_errors() {
    let host = FakeHost::new(&json!({}));
    let metadata = metadata(&host);
    let workspace = Workspace::parse(&metadata.to_string(), &host).unwrap();
    workspace
        .select(None, &BTreeSet::from(["missing".to_owned()]))
        .unwrap_err();
    workspace
        .select(
            Some(BTreeSet::from(["missing".to_owned()])),
            &BTreeSet::new(),
        )
        .unwrap_err();
    let mut missing = metadata.clone();
    missing["workspace_members"]
        .as_array_mut()
        .unwrap()
        .push(json!("missing"));
    Workspace::parse(&missing.to_string(), &host).err().unwrap();
    let mut duplicate = metadata;
    duplicate["packages"][1]["name"] = json!("library");
    Workspace::parse(&duplicate.to_string(), &host)
        .err()
        .unwrap();
}

#[test]
fn malformed_git_paths_cannot_become_package_queries_or_success_outputs() {
    for changed in ["library/file.rs", "\0", "../escape\0"] {
        let mut host = FakeHost::new(&json!({"platforms":"linux"}));
        host.event("pull_request", &event());
        host.reply("false");
        host.reply(SHA);
        host.reply(BASE);
        // Invalid Git output must still fail when there are no workspace packages to collect.
        host.reply(
            &json!({"workspace_root":host.root, "workspace_members":[], "packages":[]}).to_string(),
        );
        host.reply(host.root.to_str().unwrap());
        host.reply(changed);
        block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
        assert_eq!(
            host.processes
                .borrow()
                .last()
                .unwrap()
                .args
                .first()
                .unwrap(),
            "diff"
        );
        assert!(host.package_queries.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn absolute_git_paths_are_not_reinterpreted_as_repository_relative() {
    let mut host = FakeHost::new(&json!({"platforms":"linux"}));
    host.event("pull_request", &event());
    prepare_responses(&host, Flow::Pr);
    host.reply(host.root.to_str().unwrap());
    host.reply(&format!("{}\0", host.root.join("file.rs").display()));
    block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
    assert!(host.package_queries.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn concrete_package_names_preserve_unicode_without_csv_or_option_injection() {
    for name in ["member", "with-hyphen", "with_underscore", "caf\u{e9}"] {
        assert!(package_name(name));
    }
    for name in [
        "",
        "-option",
        "two names",
        "two,names",
        "control\u{1}",
        "tab\tname",
    ] {
        assert!(!package_name(name));
    }
}
