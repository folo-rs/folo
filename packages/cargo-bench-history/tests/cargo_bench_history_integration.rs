//! Integration tests exercising the public command surface end to end: parse
//! arguments through `argh`, translate to the typed command, and dispatch.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

use std::path::Path;

use argh::FromArgs;
use cargo_bench_history::{
    Cli, Command, MetricKind, ResultSet, RunOutcome, SCHEMA_VERSION, default_template,
    parse_config, run,
};
use serial_test::serial;

/// The committed Gungraun summary fixture used to stand in for a real run's output.
const SUMMARY_FIXTURE: &str = include_str!("fixtures/callgrind/single_unparametrized.summary.json");

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .expect("arguments should parse")
        .into_command()
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
async fn install_is_recognized_but_not_implemented() {
    let outcome = run(&command_from(&["install"])).await.unwrap();
    assert_eq!(outcome, RunOutcome::NotImplemented { command: "install" });
}

#[tokio::test]
#[cfg_attr(miri, ignore)] // Spawns a real process, which Miri cannot do.
async fn analyze_is_recognized_but_not_implemented() {
    let outcome = run(&command_from(&["analyze"])).await.unwrap();
    assert_eq!(outcome, RunOutcome::NotImplemented { command: "analyze" });
}

#[test]
fn run_forwards_passthrough_after_separator() {
    let Command::Run(options) = command_from(&["run", "--engine", "callgrind", "--", "-p", "nm"])
    else {
        panic!("expected run command");
    };
    assert_eq!(options.engine.as_deref(), Some("callgrind"));
    assert_eq!(options.passthrough, vec!["-p".to_owned(), "nm".to_owned()]);
}

#[test]
fn default_template_parses_with_two_engines() {
    let config = parse_config(default_template()).expect("template should parse");
    assert_eq!(config.engines.len(), 2);
}

/// Drives the real `run` against the real process, filesystem-harvest, and
/// local-storage adapters: the engine command is a trivial successful no-op, the
/// summary it "produced" is the committed fixture placed under the target tree,
/// and the resulting set must land in local storage parsed into the model.
#[tokio::test]
#[cfg_attr(miri, ignore)] // Spawns a real process and touches the filesystem.
#[serial] // Mutates the process-wide current directory.
async fn run_callgrind_end_to_end_stores_results() {
    let workspace = tempfile::tempdir().unwrap();
    let root = workspace.path();

    // A configuration whose engine command succeeds without doing anything; the
    // benchmark output is supplied directly below to keep the test hermetic.
    let config = "\
[project]
id = \"testproj\"

[storage.local]
path = \"./store\"

[engines.callgrind]
command = \"exit 0\"
";
    std::fs::create_dir_all(root.join(".cargo")).unwrap();
    std::fs::write(root.join(".cargo").join("bench_history.toml"), config).unwrap();

    // Stand in for the summary the engine would have written during the run. Its
    // modification time is "now", inside the harvester's freshness window.
    let summary_dir = root.join("target").join("gungraun").join("grp");
    std::fs::create_dir_all(&summary_dir).unwrap();
    std::fs::write(summary_dir.join("summary.json"), SUMMARY_FIXTURE).unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();
    let result = run(&command_from(&["run"])).await;
    // Restore the working directory before any assertion (and before the temp dir
    // is dropped, which on Windows fails while it is the current directory).
    std::env::set_current_dir(&original).unwrap();

    let outcome = result.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    let stored = single_stored_json(&root.join("store"));
    let set = ResultSet::from_json(&stored).expect("stored object should parse");
    assert_eq!(set.schema_version, SCHEMA_VERSION);
    assert_eq!(set.results.len(), 1);

    let record = &set.results[0];
    let ir = record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present");
    assert_eq!(ir.kind, MetricKind::InstructionCount);
    assert_eq!(ir.value, 36.0);
}

/// Reads the single JSON object stored under `root`, failing if there is not
/// exactly one.
fn single_stored_json(root: &Path) -> String {
    let mut found = Vec::new();
    collect_json_files(root, &mut found);
    assert_eq!(
        found.len(),
        1,
        "expected exactly one stored object: {found:?}"
    );
    std::fs::read_to_string(&found[0]).unwrap()
}

fn collect_json_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "json") {
            out.push(path);
        }
    }
}
