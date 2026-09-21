use std::path::Path;

use cbh_config::parse_config;
use serde_json::json;

use crate::action::errors::InvalidInput;
use crate::action::execute::machine_keys;
use crate::action::inputs::{ActionCommand, Inputs};
use crate::action::native::project_instance;
use crate::action::tests::fake::{SHA, analysis, report_input};

#[test]
fn strict_json_rejects_wrong_types_unknown_and_duplicate_keys() {
    for input in [
        "[]",
        "null",
        r#"{"command":true}"#,
        r#"{"command":"collect","best-of":1}"#,
        r#"{"command":"collect","command":"backfill"}"#,
        r#"{"command":"collect","install-method":""}"#,
        r#"{"command":"collect","source-path":""}"#,
        r#"{"command":"collect","unknown":""}"#,
    ] {
        let error = Inputs::parse(input.as_bytes()).unwrap_err();
        assert!(error.find_source::<InvalidInput>().is_some());
    }
}

#[test]
fn empty_known_inputs_are_unspecified_even_when_not_applicable() {
    let inputs = Inputs::parse(br#"{"command":"collect","since":"","all-features":""}"#).unwrap();
    assert!(inputs.get("since").is_none());
    assert!(inputs.boolean("all-features", true).unwrap());
}

#[test]
fn core_and_alert_commands_have_independent_input_groups() {
    for input in [
        json!({"command":"collect"}),
        json!({"command":"backfill", "from":"HEAD~2", "to":"HEAD"}),
        analysis("analyze-history"),
        analysis("analyze-pr"),
        json!({"command":"alert", "run-id":"42", "run-url":"https://github.com/o/r/actions/runs/42"}),
    ] {
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    }
}

fn publication_input_groups(sink: &str) {
    for state in ["findings", "clean", "inconclusive", "preflight", "failed"] {
        let command = format!("publish-{sink}-{state}");
        let mut input = if matches!(state, "findings" | "clean" | "inconclusive") {
            report_input(&command)
        } else {
            json!({"command":command, "head":SHA, "run-id":"42", "run-attempt":"2"})
        };
        if state == "failed" {
            input["conclusion"] = json!("cancelled");
            input["run-url"] = json!("https://github.com/o/r/actions/runs/42");
        }
        if sink == "comment" {
            input["pr-number"] = json!("7");
            if state != "failed" {
                input["packages"] = json!("crate-a,crate-b");
            }
        }
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    }
}

#[test]
fn comment_commands_have_independent_input_groups() {
    publication_input_groups("comment");
}

#[test]
fn issue_commands_have_independent_input_groups() {
    publication_input_groups("issue");
}

fn require_comment_package_scope(state: &str) {
    let command = format!("publish-comment-{state}");
    let mut input = if state == "preflight" {
        json!({"command":command})
    } else {
        report_input(&command)
    };
    input["packages"] = json!("benchmarked-crate");
    Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap();

    input.as_object_mut().unwrap().remove("packages").unwrap();
    let error = Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    assert_eq!(
        error.find_source::<InvalidInput>().unwrap().input,
        "packages"
    );
}

#[test]
fn comment_findings_require_package_scope() {
    require_comment_package_scope("findings");
}

#[test]
fn comment_clean_requires_package_scope() {
    require_comment_package_scope("clean");
}

#[test]
fn comment_preflight_requires_package_scope() {
    require_comment_package_scope("preflight");
}

#[test]
fn comment_report_inconclusive_requires_package_scope() {
    require_comment_package_scope("inconclusive");
}

#[test]
fn command_names_and_misplaced_inputs_fail_instead_of_being_ignored() {
    for command in [
        "",
        "analyze",
        "publish-comment",
        "publish-something-clean",
        "publish-issue-other",
        "publish-comment-no-data",
        "publish-issue-no-data",
        "resolve-alert",
    ] {
        Inputs::parse(&serde_json::to_vec(&json!({"command":command})).unwrap()).unwrap_err();
    }
    for (command, key) in [
        ("collect", "since"),
        ("collect", "cache"),
        ("collect", "base"),
        ("collect", "from"),
        ("backfill", "context"),
        ("analyze-history", "base"),
        ("analyze-pr", "since"),
        ("analyze-pr", "packages"),
        ("alert", "run-attempt"),
        ("alert", "body-file"),
        ("publish-issue-clean", "local-path"),
        ("publish-issue-clean", "packages"),
        ("publish-comment-failed", "packages"),
        ("publish-comment-preflight", "body-file"),
        ("publish-issue-findings", "empty-scope"),
        ("publish-issue-findings", "head"),
        ("publish-issue-inconclusive", "conclusion"),
    ] {
        let mut input = json!({"command":command});
        input[key] = json!("unexpected");
        let error = Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
        assert_eq!(error.find_source::<InvalidInput>().unwrap().input, key);
    }
}

#[test]
fn scalars_lists_and_conflicts_are_validated_before_planning() {
    for (key, value) in [
        ("all-features", "yes"),
        ("no-default-features", "True"),
        ("best-of", "0"),
        ("packages", "a,"),
        ("exclude", "-a"),
        ("bench", " , "),
        ("features", "a,,b"),
        ("on-existing", "replace"),
        ("config", "\nfile"),
        ("working-directory", " "),
        ("local-path", "a\0b"),
    ] {
        let mut input = json!({"command":"collect"});
        input[key] = json!(value);
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    }
    for input in [
        json!({"command":"collect", "packages":"a", "exclude":"b"}),
        json!({"command":"backfill", "from":"a", "to":"b", "on-existing":"error"}),
        json!({"command":"backfill", "to":"HEAD"}),
        json!({"command":"backfill", "from":"--flag", "to":"HEAD"}),
        json!({"command":"publish-issue-failed", "conclusion":"success"}),
    ] {
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    }
    let mut input = analysis("analyze-history");
    input["cache"] = json!("cache");
    input["local-path"] = json!("store");
    Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
}

#[test]
fn analysis_requires_real_keys_and_independent_platform_evidence() {
    for key in ["machine-keys", "expected-platforms", "completed-platforms"] {
        let mut input = analysis("analyze-history");
        input.as_object_mut().unwrap().remove(key).unwrap();
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    }
    let mut input = analysis("analyze-pr");
    input["completed-platforms"] = json!("macos");
    Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    assert_eq!(
        machine_keys(vec![
            b"ABCDEF0123456789\n".to_vec(),
            b"abcdef0123456789".to_vec(),
            b"0123456789abcdef".to_vec()
        ])
        .unwrap(),
        ["0123456789abcdef", "abcdef0123456789"]
    );
    for keys in [
        vec![],
        vec![b"all".to_vec()],
        vec![vec![255]],
        vec![b"0123456789abcdef\n0123456789abcdef".to_vec()],
    ] {
        machine_keys(keys).unwrap_err();
    }
}

#[test]
fn empty_scope_is_explicit_and_rejects_report_and_package_inputs() {
    let input = json!({"command":"publish-comment-inconclusive", "empty-scope":"true"});
    Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap();
    for key in [
        "body-file",
        "report-file",
        "analyzed-sha",
        "artifact-url",
        "expected-platforms",
        "completed-platforms",
        "packages",
    ] {
        let mut input = input.clone();
        input[key] = json!("unexpected");
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    }
    for key in [
        "body-file",
        "report-file",
        "analyzed-sha",
        "expected-platforms",
        "completed-platforms",
    ] {
        let mut input = report_input("publish-issue-inconclusive");
        input.as_object_mut().unwrap().remove(key).unwrap();
        Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    }
    let mut input = report_input("publish-issue-inconclusive");
    input["head"] = json!(SHA);
    let error = Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
    assert_eq!(error.find_source::<InvalidInput>().unwrap().input, "head");
}

#[test]
fn namespace_uses_core_equivalence_and_directory_fallback() {
    for (project, expected) in [
        ("MiXeD Project!", "mixed_project_"),
        ("...", "_"),
        ("", "_"),
        ("caf\u{e9}", "caf_"),
    ] {
        let config = parse_config(&format!("[project]\nid = {project:?}")).unwrap();
        assert_eq!(
            project_instance(&config, Path::new("fallback"))
                .unwrap()
                .as_str(),
            expected
        );
    }
    let config = parse_config("").unwrap();
    assert_eq!(
        project_instance(&config, Path::new("Default Checkout"))
            .unwrap()
            .as_str(),
        "default_checkout"
    );
}

#[test]
fn optional_booleans_and_lists_keep_explicit_values() {
    let inputs = Inputs::parse(
        &serde_json::to_vec(&json!({
            "command":"collect", "all-features":"false", "no-default-features":"true",
            "packages":" a,b ", "best-of":"3", "on-existing":"skip"
        }))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(inputs.command, ActionCommand::Collect);
    assert!(!inputs.boolean("all-features", true).unwrap());
    assert!(inputs.boolean("no-default-features", false).unwrap());
    assert_eq!(inputs.list("packages").unwrap(), ["a", "b"]);
    assert_eq!(inputs.get("best-of"), Some("3"));
    assert_eq!(inputs.get("on-existing"), Some("skip"));
}
