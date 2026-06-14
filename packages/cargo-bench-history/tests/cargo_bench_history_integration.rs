//! Integration tests exercising the public command surface end to end: parse
//! arguments through `argh`, translate to the typed command, and dispatch through
//! `run` against the real process, filesystem-harvest, and local-storage adapters.
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]
#![allow(
    clippy::float_cmp,
    reason = "metric values are exact integer-derived counts"
)]

use std::path::{Path, PathBuf};

use argh::FromArgs;
use cargo_bench_history::{
    Cli, Command, MetricKind, ResultRecord, ResultSet, RunError, RunOutcome, SCHEMA_VERSION,
    default_template, parse_config, run,
};
use jiff::Timestamp;
use serial_test::serial;

/// A committed Gungraun summary with no `id` (unparametrized), `Ir` = 36.
const SINGLE_SUMMARY: &str = include_str!("fixtures/callgrind/single_unparametrized.summary.json");
/// A committed Gungraun summary with `id` = `two_instants`, `Ir` = 87.
const PARAMETRIZED_SUMMARY: &str = include_str!("fixtures/callgrind/parametrized.summary.json");

/// The tool version recorded with each run. The integration test compiles within
/// the package, so its `CARGO_PKG_VERSION` matches the version `run` records.
const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .expect("arguments should parse")
        .into_command()
}

// ===========================================================================
// CLI surface (pure argument parsing; no IO, Miri-safe).
// ===========================================================================

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

#[test]
fn unknown_subcommand_is_rejected() {
    Cli::from_args(&["cargo-bench-history"], &["frobnicate"])
        .expect_err("an unknown subcommand should be rejected");
}

#[test]
fn run_rejects_unknown_flag() {
    Cli::from_args(&["cargo-bench-history"], &["run", "--frobnicate"])
        .expect_err("an unknown flag should be rejected");
}

#[test]
fn help_request_lists_subcommands() {
    let early_exit = Cli::from_args(&["cargo-bench-history"], &["--help"])
        .expect_err("help is reported as an early exit");
    assert!(
        early_exit.output.contains("run"),
        "help should list subcommands: {}",
        early_exit.output
    );
    assert!(
        early_exit.output.contains("install"),
        "{}",
        early_exit.output
    );
}

// ===========================================================================
// Dispatch of not-yet-implemented subcommands.
// ===========================================================================

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

// ===========================================================================
// Real-adapter `run` scenarios.
//
// Each drives `run` against the real process/filesystem/storage adapters from
// within a temporary workspace. They are `#[serial]` because they mutate the
// process-wide current directory, and miri-ignored because they spawn processes
// and touch the filesystem.
// ===========================================================================

/// End-to-end happy path: a successful no-op engine command, one harvested
/// summary, and a stored set with the expected object key and context.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_callgrind_end_to_end_stores_results() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);

    let outcome = workspace
        .drive(&[
            "run",
            "--target-triple",
            "x86_64-unknown-linux-gnu",
            "--timestamp",
            "2020-01-01T00:00:00Z",
        ])
        .await
        .expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    let (key, set) = workspace.single_object();

    // Synthetic partition (Callgrind is hardware-independent) under the resolved
    // triple and the backfilled effective second; the run-id suffix is dynamic.
    let effective: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
    let prefix = format!(
        "v1/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{}-unknown-",
        effective.as_second()
    );
    assert!(key.starts_with(&prefix), "unexpected key: {key}");
    assert!(key.ends_with(".json"), "unexpected key: {key}");

    assert_eq!(set.schema_version, SCHEMA_VERSION);
    assert_eq!(
        set.context.toolchain.target_triple,
        "x86_64-unknown-linux-gnu"
    );
    assert_eq!(set.context.tool_version, TOOL_VERSION);
    assert_eq!(set.context.timestamps.effective, effective);

    assert_eq!(set.results.len(), 1);
    let record = &set.results[0];
    assert!(record.id.value.is_none());
    assert_eq!(ir_of(record), 36.0);
    let ir = record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present");
    assert_eq!(ir.kind, MetricKind::InstructionCount);
}

/// Two summaries under the target tree yield one stored set with one record each.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_stores_a_record_per_summary() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));
    workspace.place_callgrind_summary("a", SINGLE_SUMMARY);
    workspace.place_callgrind_summary("b", PARAMETRIZED_SUMMARY);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("covering 2"), "{message}");

    let (_, set) = workspace.single_object();
    assert_eq!(set.results.len(), 2);

    let parametrized = set
        .results
        .iter()
        .find(|record| record.id.value.as_deref() == Some("two_instants"))
        .expect("parametrized record should be present");
    assert_eq!(ir_of(parametrized), 87.0);

    let unparametrized = set
        .results
        .iter()
        .find(|record| record.id.value.is_none())
        .expect("unparametrized record should be present");
    assert_eq!(ir_of(unparametrized), 36.0);
}

/// `--no-store` still harvests the output but writes nothing to storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_no_store_harvests_without_storing() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);

    let outcome = workspace
        .drive(&["run", "--no-store"])
        .await
        .expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Harvested 1"), "{message}");
    assert!(message.contains("nothing stored"), "{message}");

    assert!(
        workspace.stored_objects().is_empty(),
        "nothing should be stored"
    );
}

/// A non-zero engine exit aborts the run with the engine name and exit code, and
/// nothing is stored.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_propagates_nonzero_engine_exit() {
    let workspace = Workspace::new(&callgrind_config("exit 7"));

    let error = workspace
        .drive(&["run"])
        .await
        .expect_err("a non-zero engine exit should fail the run");
    let RunError::Engine { engine, code } = error else {
        panic!("expected an engine error, got {error:?}");
    };
    assert_eq!(engine, "callgrind");
    assert_eq!(code, Some(7));

    assert!(
        workspace.stored_objects().is_empty(),
        "a failed run stores nothing"
    );
}

/// Requesting an engine that is not a known engine system is rejected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_rejects_unknown_requested_engine() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));

    let error = workspace
        .drive(&["run", "--engine", "dhat"])
        .await
        .expect_err("an unknown engine should be rejected");
    let RunError::NoEngine { message } = error else {
        panic!("expected a no-engine error, got {error:?}");
    };
    assert!(message.contains("unknown engine"), "{message}");
}

/// Requesting a known engine that the configuration does not list is rejected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_rejects_unconfigured_requested_engine() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));

    let error = workspace
        .drive(&["run", "--engine", "criterion"])
        .await
        .expect_err("an unconfigured engine should be rejected");
    let RunError::NoEngine { message } = error else {
        panic!("expected a no-engine error, got {error:?}");
    };
    assert!(message.contains("not configured"), "{message}");
}

/// Requesting a configured engine this iteration cannot harvest is rejected.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_rejects_unsupported_requested_engine() {
    let workspace = Workspace::new(&criterion_config("exit 0"));

    let error = workspace
        .drive(&["run", "--engine", "criterion"])
        .await
        .expect_err("an unsupported engine should be rejected");
    let RunError::NoEngine { message } = error else {
        panic!("expected a no-engine error, got {error:?}");
    };
    assert!(message.contains("not supported"), "{message}");
}

/// Without an explicit `--engine`, an unsupported configured engine is skipped
/// while the supported one runs and is reported.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_skips_unsupported_configured_engine() {
    let workspace = Workspace::new(CALLGRIND_AND_CRITERION_CONFIG);
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);

    let outcome = workspace.drive(&["run"]).await.expect("run should succeed");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(
        message.contains("Skipped unsupported engine(s): criterion"),
        "{message}"
    );
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// Explicitly selecting the supported engine works end to end.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_explicit_callgrind_selection_stores_results() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);

    let outcome = workspace
        .drive(&["run", "--engine", "callgrind"])
        .await
        .expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// `--config` loads the configuration from a non-default path.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_uses_explicit_config_path() {
    // Only the custom path holds a configuration; the default discovery path is
    // absent, so a successful run proves `--config` was honored.
    let workspace = Workspace::with_config_at("config/bench.toml", &callgrind_config("exit 0"));
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);

    let outcome = workspace
        .drive(&["run", "--config", "config/bench.toml"])
        .await
        .expect("run should succeed");
    assert!(matches!(outcome, RunOutcome::Completed { .. }));
    assert_eq!(workspace.stored_objects().len(), 1);
}

/// Two sequential runs store two distinct objects without clobbering.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn two_runs_store_two_distinct_objects() {
    let workspace = Workspace::new(&callgrind_config("exit 0"));

    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);
    workspace
        .drive(&["run"])
        .await
        .expect("first run should succeed");

    // Refresh the summary so its modification time stays inside the second run's
    // harvest window.
    workspace.place_callgrind_summary("grp", SINGLE_SUMMARY);
    workspace
        .drive(&["run"])
        .await
        .expect("second run should succeed");

    let objects = workspace.stored_objects();
    assert_eq!(objects.len(), 2, "each run should store a distinct object");
    assert_ne!(objects[0].0, objects[1].0, "object keys must differ");
}

// ===========================================================================
// Test harness and helpers.
// ===========================================================================

/// A hermetic workspace for driving `run` against the real adapters.
///
/// Writes a configuration and (optionally) fake engine output under a temporary
/// directory, then drives a command with that directory as the process current
/// directory, restoring the original directory before returning so assertions and
/// temp-dir cleanup happen outside it (required on Windows).
struct Workspace {
    dir: tempfile::TempDir,
}

impl Workspace {
    /// Creates a workspace with `config` at the default `.cargo/bench_history.toml`.
    fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        };
        let cargo_dir = workspace.root().join(".cargo");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(cargo_dir.join("bench_history.toml"), config).unwrap();
        workspace
    }

    /// Creates a workspace with `config` at a non-default `relative` path.
    fn with_config_at(relative: &str, config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        };
        let path = workspace.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config).unwrap();
        workspace
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Writes `content` as a Callgrind `summary.json` under a fresh target group,
    /// giving it a current modification time inside the harvester's window.
    fn place_callgrind_summary(&self, group: &str, content: &str) {
        let dir = self.root().join("target").join("gungraun").join(group);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary.json"), content).unwrap();
    }

    /// Drives `run` with `args` from inside this workspace.
    async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(self.root()).unwrap();
        let result = run(&command_from(args)).await;
        // Restore the working directory before any assertion (and before the temp
        // dir is dropped, which on Windows fails while it is the current directory).
        std::env::set_current_dir(&original).unwrap();
        result
    }

    /// All stored objects as `(object key, parsed result set)` pairs, sorted by key.
    fn stored_objects(&self) -> Vec<(String, ResultSet)> {
        let store = self.root().join("store");
        let mut files = Vec::new();
        collect_json_files(&store, &mut files);

        let mut objects: Vec<(String, ResultSet)> = files
            .into_iter()
            .map(|path| {
                let key = path
                    .strip_prefix(&store)
                    .unwrap()
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                let set = ResultSet::from_json(&std::fs::read_to_string(&path).unwrap())
                    .expect("stored object should parse");
                (key, set)
            })
            .collect();
        objects.sort_by(|left, right| left.0.cmp(&right.0));
        objects
    }

    /// Returns the single stored object, failing if there is not exactly one.
    fn single_object(&self) -> (String, ResultSet) {
        let mut objects = self.stored_objects();
        assert_eq!(
            objects.len(),
            1,
            "expected exactly one stored object, found {}",
            objects.len()
        );
        objects.pop().unwrap()
    }
}

/// The `Ir` (instruction count) metric value of a record.
fn ir_of(record: &ResultRecord) -> f64 {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .expect("instruction count should be present")
        .value
}

/// A single-engine Callgrind configuration whose command is `command`.
fn callgrind_config(command: &str) -> String {
    format!(
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n\n\
         [engines.callgrind]\n\
         command = \"{command}\"\n"
    )
}

/// A single-engine Criterion configuration whose command is `command`.
fn criterion_config(command: &str) -> String {
    format!(
        "[project]\n\
         id = \"testproj\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n\n\
         [engines.criterion]\n\
         command = \"{command}\"\n"
    )
}

/// A configuration with both a supported (callgrind) and an unsupported
/// (criterion) engine; the criterion command never runs because it is skipped.
const CALLGRIND_AND_CRITERION_CONFIG: &str = "\
[project]
id = \"testproj\"

[storage.local]
path = \"./store\"

[engines.callgrind]
command = \"exit 0\"

[engines.criterion]
command = \"exit 99\"
";

fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
