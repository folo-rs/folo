#![allow(
    clippy::indexing_slicing,
    reason = "Tests mutate fixed JSON fixture shapes to exercise validation and literal rendering."
)]

use cbh_model::{BackfillCompletion, BackfillCounts, BackfillStop};
use serde_json::{Value, json};

use crate::result::tests::evidence;
use crate::result::{AnalysisMode, Outcome};
use crate::summary::input::SummaryInput;
use crate::summary::render::{InvalidSummary, render};

/// Supplies an analyzer-shaped document without external I/O.
fn report(outcome: Outcome, partial: bool) -> Value {
    let evidence = evidence(AnalysisMode::History, outcome, true);
    let report = evidence.report;
    let coverage = match outcome {
        Outcome::Findings if partial => "partial",
        Outcome::Findings | Outcome::Clean => "full",
        Outcome::InsufficientBaseline => "nothing_judged",
        Outcome::NothingInScope => "no_series",
        Outcome::Partial => "partial",
    };
    json!({
        "tip_commit": report.commit.as_str(), "tip_dirty": false, "mode":"history",
        "outcome":outcome.as_str(), "notable":outcome == Outcome::Findings,
        "census":{"coverage":coverage, "judged":report.judged,
            "in_scope":if partial { 2 } else { report.in_scope }},
        "sets":[{"engine":"criterion", "target_triple":"x86_64-unknown-linux-gnu",
            "machine_key":"0123456789abcdef"}]
    })
}

/// Assembles invocation facts independently of benchmark evidence.
fn input(result: Value) -> Value {
    let mut input = json!({
        "execution":"success", "publication":"disabled",
        "run_url":"https://github.com/o/r/actions/runs/7"
    });
    input["result"] = result;
    input
}

fn analysis() -> Value {
    input(json!({
        "kind":"analysis", "report_file":"report.json",
        "analyzed_sha":"a".repeat(40), "expected_platforms":"linux,windows",
        "completed_platforms":"linux,windows", "packages":["one","two"]
    }))
}

fn render_report(input: Value, report: &Value) -> String {
    let report = serde_json::to_vec(report).unwrap();
    render(serde_json::from_value(input).unwrap(), |_| {
        Ok(report.clone())
    })
    .unwrap()
}

#[test]
fn all_analysis_outcomes_keep_execution_publication_and_coverage_separate() {
    for (outcome, expected) in [
        (Outcome::Findings, "Notable benchmark changes"),
        (Outcome::Clean, "No notable changes detected across"),
        (Outcome::Partial, "among the series that could be judged"),
        (Outcome::InsufficientBaseline, "Insufficient evidence"),
        (Outcome::NothingInScope, "No metric series"),
    ] {
        let card = render_report(analysis(), &report(outcome, false));
        assert!(card.contains(expected));
        assert!(card.contains("| Execution | Completed |"));
        assert!(card.contains("Disabled; collection and storage are independent"));
        assert!(card.contains("in-scope metric series"));
        assert!(card.contains("criterion"));
        assert!(card.contains("0123456789abcdef"));
        assert!(card.contains("**Next action:**"));
        assert!(card.contains("Live-head freshness not checked"));
    }
}

#[test]
fn missing_platforms_prevent_clean_disposition_without_hiding_findings() {
    for outcome in [Outcome::Findings, Outcome::Clean] {
        let mut input = analysis();
        input["result"]["completed_platforms"] = json!("linux");
        input["execution"] = json!("failure");
        let card = render_report(input, &report(outcome, outcome == Outcome::Findings));
        assert!(card.contains("| Execution | Failed |"));
        assert!(card.contains("Missing platforms | <code>windows</code>"));
        assert!(card.contains("Incomplete platform coverage"));
        if outcome == Outcome::Findings {
            assert!(card.contains("| Report disposition | findings |"));
            assert!(card.contains("Incomplete series evidence"));
        } else {
            assert!(card.contains("| Report disposition | inconclusive |"));
            assert!(!card.contains("| Report disposition | clean |"));
        }
    }
}

#[test]
fn baseline_guidance_requires_distinct_history_not_a_promised_verdict() {
    let card = render_report(analysis(), &report(Outcome::InsufficientBaseline, false));
    assert!(card.contains("distinct comparable commits"));
    assert!(card.contains("same commit does not add independent history"));
    assert!(card.contains("does not guarantee"));
}

#[test]
fn empty_collection_diagnostics_are_literal_and_do_not_guess_the_cause() {
    let mut evidence = report(Outcome::NothingInScope, false);
    evidence["hint"] = json!("No selected runs at <root>|[link](url)\nextra");
    let card = render_report(analysis(), &evidence);
    assert!(card.contains("&lt;root&gt;&#124;&#91;link&#93;"));
    assert!(!card.contains("\nextra"));
    assert!(card.contains("empty output alone does not identify"));
    assert!(card.contains("artifact roots"));
}

#[test]
fn freshness_requires_an_observed_head_and_publication_completion_is_not_a_write_claim() {
    for (head, expected) in [
        ("a".repeat(40), "Matches the observed head"),
        ("b".repeat(40), "Out of date relative"),
    ] {
        let mut input = analysis();
        input["result"]["observed_head"] = json!(head);
        input["publication"] = json!("completed");
        let card = render_report(input, &report(Outcome::Clean, false));
        assert!(card.contains(expected));
        assert!(card.contains("may leave posts unchanged"));
    }
}

#[test]
fn unavailable_reports_never_invent_a_verdict_for_any_execution_state() {
    for execution in ["success", "failure", "cancelled", "skipped"] {
        let mut input = input(json!({"kind":"unavailable", "reason":"reports_unavailable"}));
        input["execution"] = json!(execution);
        let card = render(serde_json::from_value(input).unwrap(), |_| {
            panic!("unavailable results must not read evidence")
        })
        .unwrap();
        assert!(card.contains("no benchmark verdict"));
        assert!(card.contains("No validated analysis report"));
        assert!(!card.contains("No notable changes"));
    }
}

#[test]
fn explicit_no_work_reasons_remain_distinct() {
    for (reason, expected) in [
        ("empty_scope", "No benchmarkable package"),
        ("fork_pull_request", "Fork pull-request policy"),
        ("no_eligible_commit", "endpoint age policy"),
        ("collection_only", "collected measurements"),
    ] {
        let card = render(
            serde_json::from_value(input(json!({"kind":"unavailable","reason":reason}))).unwrap(),
            |_| panic!("no analysis was requested"),
        )
        .unwrap();
        assert!(card.contains(expected));
        assert!(card.contains("no benchmark verdict"));
    }
}

#[test]
fn missing_or_invalid_supplied_evidence_is_an_error_not_a_no_work_fallback() {
    let input: SummaryInput = serde_json::from_value(analysis()).unwrap();
    let error = render(input, |_| Err(InvalidSummary::new().into())).unwrap_err();
    assert!(error.find_source::<InvalidSummary>().is_some());
    let mut value = analysis();
    value["execution"] = json!("skipped");
    render(serde_json::from_value(value).unwrap(), |_| Ok(Vec::new())).unwrap_err();
    let mut evidence = report(Outcome::Clean, false);
    evidence["census"]["judged"] = json!(0);
    render(serde_json::from_value(analysis()).unwrap(), |_| {
        Ok(evidence.to_string().into_bytes())
    })
    .unwrap_err();
}

fn backfill_input() -> Value {
    input(json!({
        "kind":"backfill","from":"a".repeat(40),"to":"b".repeat(40),
        "expected_platforms":"linux,windows,macos",
        "platforms":[
            {"platform":"linux","execution":"success","completion_file":"completion.json"},
            {"platform":"windows","execution":"cancelled"}
        ]
    }))
}

#[test]
fn bounded_backfill_retains_counts_missing_legs_and_safe_continuation() {
    let completion = BackfillCompletion::new(
        "a".repeat(40),
        "b".repeat(40),
        5,
        BackfillCounts {
            stored: 1,
            skipped: 1,
            empty: 1,
            failed: 1,
            deferred: 1,
        },
        BackfillStop::CommitLimit,
        true,
    )
    .unwrap();
    let card = render(serde_json::from_value(backfill_input()).unwrap(), |_| {
        Ok(completion.to_json().unwrap().into_bytes())
    })
    .unwrap();
    assert!(card.contains("| linux | Completed | 1 | 1 | 1 | 1 | 1 | Commit limit reached"));
    assert!(card.contains("| windows | Cancelled | - | - | - | - | - | Completion unavailable"));
    assert!(card.contains("| macos | Unavailable | - | - | - | - | - | No platform result"));
    assert!(card.contains("collection-only backfill has no benchmark verdict"));
    assert!(card.contains("same frozen range"));
    assert!(card.contains("skip-existing"));
    assert!(card.contains("repeating overwrite starts"));
}

#[test]
fn backfill_rejects_duplicate_unexpected_mismatched_or_contradictory_evidence() {
    let completion = BackfillCompletion::new(
        "a".repeat(40),
        "b".repeat(40),
        1,
        BackfillCounts {
            failed: 1,
            ..BackfillCounts::default()
        },
        BackfillStop::BenchmarkFailure,
        false,
    )
    .unwrap()
    .to_json()
    .unwrap();
    for mutation in ["success", "range", "duplicate", "unknown", "publication"] {
        let mut input = backfill_input();
        input["result"]["platforms"][0]["execution"] = json!("failure");
        match mutation {
            "success" => input["result"]["platforms"][0]["execution"] = json!("success"),
            "range" => input["result"]["from"] = json!("c".repeat(40)),
            "duplicate" => input["result"]["platforms"][1]["platform"] = json!("linux"),
            "unknown" => input["result"]["platforms"][1]["platform"] = json!("other"),
            "publication" => input["publication"] = json!("completed"),
            _ => unreachable!(),
        }
        render(serde_json::from_value(input).unwrap(), |_| {
            Ok(completion.as_bytes().to_vec())
        })
        .unwrap_err();
    }
}

#[test]
fn backfill_exhaustion_and_stopping_failure_are_not_benchmark_verdicts() {
    for (reason, execution, expected) in [
        (BackfillStop::RangeExhausted, "success", "Range exhausted"),
        (
            BackfillStop::BenchmarkFailure,
            "failure",
            "Stopped on benchmark failure",
        ),
    ] {
        let completion = BackfillCompletion::new(
            "a".repeat(40),
            "b".repeat(40),
            1,
            BackfillCounts {
                failed: 1,
                ..BackfillCounts::default()
            },
            reason,
            false,
        )
        .unwrap();
        let mut input = backfill_input();
        input["result"]["platforms"][0]["execution"] = json!(execution);
        let card = render(serde_json::from_value(input).unwrap(), |_| {
            Ok(completion.to_json().unwrap().into_bytes())
        })
        .unwrap();
        assert!(card.contains(expected));
        assert!(card.contains("no benchmark verdict"));
    }
}

#[test]
fn unsafe_links_and_unknown_input_fields_are_rejected() {
    for url in [
        "javascript:alert(1)",
        "https://user:pass@example.com",
        "https://example.com\ninjected",
    ] {
        let mut input = input(json!({"kind":"unavailable", "reason":"reports_unavailable"}));
        input["run_url"] = json!(url);
        render(serde_json::from_value(input).unwrap(), |_| panic!()).unwrap_err();
    }

    let mut value = analysis();
    value["extra"] = json!(true);
    serde_json::from_value::<SummaryInput>(value).unwrap_err();
}

#[test]
fn inventories_remain_compact_and_failed_publication_does_not_change_analysis() {
    let mut input = analysis();
    input["result"]["packages"] = json!(["first_name", "<second>", "third|name", "fourth-hidden"]);
    input["publication"] = json!("failed");
    input["artifact_url"] = json!("https://example.com/artifacts/(report)");
    let card = render_report(input, &report(Outcome::Clean, false));
    assert!(card.contains("| Report disposition | clean |"));
    assert!(card.contains("| GitHub publication | Failed;"));
    assert!(card.contains("first&#95;name"));
    assert!(card.contains("&lt;second&gt;"));
    assert!(card.contains("third&#124;name"));
    assert!(card.contains("4 total"));
    assert!(!card.contains("fourth-hidden"));
    assert!(card.contains("/%28report%29"));
}
