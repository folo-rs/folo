#![allow(
    clippy::indexing_slicing,
    reason = "These tests mutate known JSON fixtures to exercise individual validation paths."
)]

use std::collections::BTreeSet;

use futures::executor::block_on;
use serde_json::{Value, json};

use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::port::Output;
use crate::action::preparation::inputs::{WorkflowInputs, package_name};
use crate::action::preparation::scope::Workspace;
use crate::action::preparation::{Flow, PrepareWorkflowArgs, prepare_with};
use crate::action::tests::fake::{FakeHost, SHA};

const BASE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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
        WorkflowInputs::parse(json.as_bytes()).err().unwrap();
    }
    let inputs = WorkflowInputs::parse(
        br#"{"platforms":"linux","config":"","exclude":" library,library "}"#,
    )
    .unwrap();
    assert_eq!(inputs.excluded, BTreeSet::from(["library".to_owned()]));
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
    for name in ["deleted.rs", "new.rs"] {
        host.packages.insert(
            host.root.join("library").join(name),
            Some("library".to_owned()),
        );
    }
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
        prepare_responses(&host, Flow::Pr);
        host.reply(host.root.to_str().unwrap());
        host.reply(changed);
        block_on(prepare_with(args(&host, Flow::Pr), &host)).unwrap_err();
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
