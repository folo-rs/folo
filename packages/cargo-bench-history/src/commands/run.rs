//! The `run` command: execute the configured engines and store the results.
//!
//! Orchestration is generic over a set of small async ports (process runner,
//! environment probe, benchmark-output source, storage) plus an injected
//! [`tick::Clock`], so the whole flow is exercised in-process with fakes. The
//! public [`execute`] wires the real adapters and is what the binary runs.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use jiff::Timestamp;
use tick::Clock;

use crate::bench::{injected_env, parse_callgrind_summary};
use crate::bench_output::{BenchOutputSource, FsBenchOutputSource};
use crate::comparability::{ComparabilityKey, EngineSystem, resolve_target_triple};
use crate::config::{Config, load_config};
use crate::context::{
    CiInfo, RunContext, Timestamps, ToolchainInfo, detect_ci, resolve_effective_time,
};
use crate::git::GitSnapshot;
use crate::host::RustcInfo;
use crate::model::ResultSet;
use crate::probe::{EnvironmentProbe, SystemProbe};
use crate::process::{BenchRunner, TokioBenchRunner};
use crate::storage::Storage;
use crate::wiring::{build_local_storage, default_config_path, resolve_project_id};
use crate::{RunError, RunOptions, RunOutcome};

/// The injected collaborators an orchestrated run operates against.
pub(crate) struct RunDeps<'a, R, P, O, S> {
    /// Launches engine commands.
    pub(crate) runner: &'a R,
    /// Probes git and toolchain facts.
    pub(crate) probe: &'a P,
    /// Harvests engine output.
    pub(crate) output: &'a O,
    /// Persists result sets.
    pub(crate) storage: &'a S,
    /// Supplies wall-clock time.
    pub(crate) clock: &'a Clock,
    /// Resolves environment variables (for CI detection).
    pub(crate) env: &'a dyn Fn(&str) -> Option<String>,
    /// The loaded configuration.
    pub(crate) config: &'a Config,
    /// Resolved project identity for the storage partition.
    pub(crate) project_id: &'a str,
    /// Version of this tool, recorded with each run.
    pub(crate) tool_version: &'a str,
    /// Unique identifier distinguishing this run's stored objects.
    pub(crate) run_id: &'a str,
}

/// The real `run`: wire the production adapters and orchestrate.
///
/// `target_root` overrides the cargo target directory the harvest scans.
/// Production passes `None`, resolving the root from `CARGO_TARGET_DIR` (or the
/// `target/` default). Tests pass an explicit root so the harvest is hermetic
/// without mutating the process environment.
pub(crate) async fn execute(
    options: &RunOptions,
    target_root: Option<PathBuf>,
) -> Result<RunOutcome, RunError> {
    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    let storage = build_local_storage(&config);

    let runner = TokioBenchRunner;
    let probe = SystemProbe;
    let output = FsBenchOutputSource::new(target_root.unwrap_or_else(resolve_target_root));
    let clock = Clock::new_tokio();
    let env = |name: &str| std::env::var(name).ok();
    let run_id = generate_run_id(&clock);

    let deps = RunDeps {
        runner: &runner,
        probe: &probe,
        output: &output,
        storage: &storage,
        clock: &clock,
        env: &env,
        config: &config,
        project_id: &project_id,
        tool_version: env!("CARGO_PKG_VERSION"),
        run_id: &run_id,
    };

    execute_run(options, &deps).await
}

/// The engines a run will execute, plus those skipped as unsupported.
#[derive(Debug)]
struct EngineSelection {
    /// Engines that will run, as `(config key, engine)` pairs.
    to_run: Vec<(String, EngineSystem)>,
    /// Names of configured engines skipped because this iteration cannot harvest
    /// their output yet.
    skipped: Vec<String>,
}

/// Probe facts shared across every engine in a single run.
struct SharedContext {
    /// Git snapshot of the working directory.
    git: GitSnapshot,
    /// Active toolchain facts.
    rustc: RustcInfo,
    /// Detected CI environment.
    ci: CiInfo,
    /// Host triple (possibly differing from the recorded target triple).
    host_triple: String,
}

/// The result of processing one engine.
struct EngineSummary {
    /// Whether a result set was stored.
    stored: bool,
    /// Number of benchmark cases harvested.
    count: usize,
    /// Human-readable per-engine summary.
    label: String,
}

/// Orchestrates a run against injected collaborators.
pub(crate) async fn execute_run<R, P, O, S>(
    options: &RunOptions,
    deps: &RunDeps<'_, R, P, O, S>,
) -> Result<RunOutcome, RunError>
where
    R: BenchRunner,
    P: EnvironmentProbe,
    O: BenchOutputSource,
    S: Storage,
{
    let selection = resolve_engines(deps.config, options.engine.as_deref())?;

    let rustc = deps.probe.toolchain().await?;
    let shared = SharedContext {
        git: deps.probe.git().await?,
        host_triple: rustc.host.clone().unwrap_or_default(),
        rustc,
        ci: detect_ci(deps.env),
    };

    let mut stored = 0_usize;
    let mut harvested = 0_usize;
    let mut labels = Vec::new();

    for (name, engine) in &selection.to_run {
        let summary = process_engine(options, deps, &shared, name, *engine).await?;
        if summary.stored {
            stored = stored.saturating_add(1);
        }
        harvested = harvested.saturating_add(summary.count);
        labels.push(summary.label);
    }

    let message = build_message(
        options.no_store,
        stored,
        harvested,
        &labels,
        &selection.skipped,
    );
    Ok(RunOutcome::Completed { message })
}

/// Runs one engine, harvests its output, and (unless suppressed) stores the set.
async fn process_engine<R, P, O, S>(
    options: &RunOptions,
    deps: &RunDeps<'_, R, P, O, S>,
    shared: &SharedContext,
    name: &str,
    engine: EngineSystem,
) -> Result<EngineSummary, RunError>
where
    R: BenchRunner,
    P: EnvironmentProbe,
    O: BenchOutputSource,
    S: Storage,
{
    let engine_config = deps
        .config
        .engines
        .get(name)
        .expect("engine name was taken from the configuration map");

    let run_start = deps.clock.system_time();
    let injected = injected_env(engine);
    let argv = build_command_line(
        &engine_config.command,
        &engine_config.extra_args,
        &options.passthrough,
    )
    .map_err(|message| RunError::Command {
        engine: name.to_owned(),
        message,
    })?;

    let status = deps.runner.run_engine(&argv, &injected).await?;
    if !status.success {
        return Err(RunError::Engine {
            engine: name.to_owned(),
            code: status.code,
        });
    }

    let raw = deps.output.collect(engine, run_start).await?;
    let mut records = Vec::with_capacity(raw.len());
    for summary in &raw {
        let record =
            parse_callgrind_summary(&summary.content).map_err(|error| RunError::Parse {
                message: error.to_string(),
            })?;
        records.push(record);
    }
    let count = records.len();

    let execution = timestamp_from(run_start);
    let ingest: Timestamp = deps.clock.system_time_as();
    let effective = resolve_effective_time(options.timestamp, shared.git.committer, ingest);
    let target_triple = resolve_target_triple(
        options.target_triple.as_deref(),
        engine,
        &shared.host_triple,
    );

    let context = RunContext::new(
        Timestamps::new(effective, execution, ingest),
        shared.git.info.clone(),
        shared.ci.clone(),
        ToolchainInfo {
            target_triple: target_triple.clone(),
            host_triple: shared.host_triple.clone(),
            rustc_version: shared.rustc.version.clone(),
        },
        deps.tool_version.to_owned(),
    );
    let result_set = ResultSet::new(context, records);

    if options.no_store {
        return Ok(EngineSummary {
            stored: false,
            count,
            label: format!("{name}: {count} harvested (not stored)"),
        });
    }

    // Callgrind is hardware-independent, so its partition uses no machine key.
    // Hardware fingerprinting arrives with the Criterion adapter.
    let key = ComparabilityKey::new(deps.project_id.to_owned(), engine, target_triple, None);
    let short_commit = shared.git.info.short_commit.as_deref().unwrap_or("unknown");
    let object_key = key.object_key(effective.as_second(), short_commit, deps.run_id);

    // A freshly built result set is composed of plain structs and finite counts,
    // so serialization cannot fail.
    let json = result_set
        .to_json()
        .expect("a freshly built result set always serializes to JSON");
    deps.storage.put(&object_key, json.as_bytes()).await?;

    Ok(EngineSummary {
        stored: true,
        count,
        label: format!("{name}: {count} stored"),
    })
}

/// Decides which engines to run from the configuration and an optional filter.
fn resolve_engines(config: &Config, requested: Option<&str>) -> Result<EngineSelection, RunError> {
    let mut configured = Vec::new();
    for name in config.engines.keys() {
        let engine = EngineSystem::from_name(name).ok_or_else(|| RunError::NoEngine {
            message: format!("configuration references unknown engine {name:?}"),
        })?;
        configured.push((name.clone(), engine));
    }

    let candidates = match requested {
        Some(requested) => {
            let engine = EngineSystem::from_name(requested).ok_or_else(|| RunError::NoEngine {
                message: format!("unknown engine {requested:?}"),
            })?;
            if !configured.iter().any(|(_, candidate)| *candidate == engine) {
                return Err(RunError::NoEngine {
                    message: format!("engine {requested:?} is not configured"),
                });
            }
            if !is_supported(engine) {
                return Err(RunError::NoEngine {
                    message: format!(
                        "engine {requested:?} is not supported yet (this iteration supports callgrind)"
                    ),
                });
            }
            configured
                .into_iter()
                .filter(|(_, candidate)| *candidate == engine)
                .collect()
        }
        None => configured,
    };

    let mut to_run = Vec::new();
    let mut skipped = Vec::new();
    for (name, engine) in candidates {
        if is_supported(engine) {
            to_run.push((name, engine));
        } else {
            skipped.push(name);
        }
    }

    if to_run.is_empty() {
        return Err(RunError::NoEngine {
            message: if skipped.is_empty() {
                "no engines are configured".to_owned()
            } else {
                format!(
                    "only unsupported engines are configured: {} \
                     (this iteration supports callgrind)",
                    skipped.join(", ")
                )
            },
        });
    }

    Ok(EngineSelection { to_run, skipped })
}

/// Whether this iteration can harvest `engine`'s output.
fn is_supported(engine: EngineSystem) -> bool {
    matches!(engine, EngineSystem::Callgrind)
}

/// Tokenizes the engine command and appends its configured and forwarded
/// arguments verbatim, producing a full argv for direct (shell-free) execution.
///
/// The configured `command` is split with POSIX shell-word rules (honoring
/// quotes), then `extra_args` and `passthrough` are appended as-is — each is
/// already a discrete argument and is forwarded without re-tokenization, so
/// values containing spaces or quotes are preserved exactly. Returns an error if
/// `command` is not validly quoted or yields no program to run.
fn build_command_line(
    command: &str,
    extra_args: &[String],
    passthrough: &[String],
) -> Result<Vec<String>, String> {
    let mut argv = shlex::split(command)
        .ok_or_else(|| format!("command {command:?} is not a valid shell-quoted string"))?;
    if argv.is_empty() {
        return Err("command is empty".to_owned());
    }
    argv.extend(extra_args.iter().cloned());
    argv.extend(passthrough.iter().cloned());
    Ok(argv)
}

/// Builds the human-readable run summary.
fn build_message(
    no_store: bool,
    stored: usize,
    harvested: usize,
    labels: &[String],
    skipped: &[String],
) -> String {
    let mut message = if no_store {
        format!("Harvested {harvested} benchmark case(s); nothing stored (--no-store).")
    } else {
        format!("Stored {stored} result set(s) covering {harvested} benchmark case(s).")
    };
    if !labels.is_empty() {
        message.push_str(" [");
        message.push_str(&labels.join("; "));
        message.push(']');
    }
    if !skipped.is_empty() {
        message.push_str(" Skipped unsupported engine(s): ");
        message.push_str(&skipped.join(", "));
        message.push('.');
    }
    message
}

/// Converts a [`SystemTime`] to a [`Timestamp`].
fn timestamp_from(time: SystemTime) -> Timestamp {
    Timestamp::try_from(time).expect("system time is within the supported timestamp range")
}

/// The cargo target directory, honoring `CARGO_TARGET_DIR`.
fn resolve_target_root() -> PathBuf {
    target_root_from(std::env::var_os("CARGO_TARGET_DIR"))
}

/// Resolves the cargo target directory from an optional `CARGO_TARGET_DIR` value,
/// falling back to the conventional `target/` directory when it is unset.
fn target_root_from(configured: Option<std::ffi::OsString>) -> PathBuf {
    configured.map_or_else(|| PathBuf::from("target"), PathBuf::from)
}

/// Generates a run identifier unique within a partition.
///
/// Combines the process id, clock nanoseconds, and a process-global monotonic
/// sequence number so that back-to-back runs in the same process never collide
/// even when the system clock has coarse resolution (which would otherwise let
/// `{pid}-{nanos}` repeat and trip the write-once storage contract).
fn generate_run_id(clock: &Clock) -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    let nanos = clock
        .system_time()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{nanos}-{sequence}", std::process::id())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::executor::block_on;

    use super::*;
    use crate::bench_output::RawSummary;
    use crate::config::parse_config;
    use crate::git::build_snapshot;
    use crate::process::EngineStatus;
    use crate::storage::MemoryStorage;

    const SINGLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/single_unparametrized.summary.json");
    const PARAMETRIZED_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/parametrized.summary.json");

    /// The frozen wall-clock instant used by orchestration tests (2023-11-14Z).
    const FROZEN_UNIX: u64 = 1_700_000_000;

    fn frozen_time() -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(FROZEN_UNIX))
            .expect("frozen instant is within range")
    }

    #[test]
    fn run_ids_are_unique_even_with_an_identical_clock_reading() {
        // A frozen clock returns the same instant on every read, so the pid and
        // nanos components are identical; the monotonic sequence is what keeps
        // back-to-back run ids (and thus object keys) from colliding.
        let clock = Clock::new_frozen_at(frozen_time());
        let first = generate_run_id(&clock);
        let second = generate_run_id(&clock);
        assert_ne!(first, second);
    }

    #[test]
    fn target_root_falls_back_to_the_conventional_directory() {
        assert_eq!(target_root_from(None), PathBuf::from("target"));
    }

    #[test]
    fn target_root_honors_an_explicit_cargo_target_dir() {
        let configured = std::ffi::OsString::from("/custom/out");
        assert_eq!(
            target_root_from(Some(configured)),
            PathBuf::from("/custom/out")
        );
    }

    #[test]
    fn resolve_target_root_matches_the_ambient_environment() {
        // Resolving never panics and agrees with `target_root_from` for whatever
        // `CARGO_TARGET_DIR` happens to be set (or unset) in this process.
        assert_eq!(
            resolve_target_root(),
            target_root_from(std::env::var_os("CARGO_TARGET_DIR"))
        );
    }

    #[test]
    fn timestamp_from_preserves_the_instant() {
        let timestamp = timestamp_from(frozen_time());
        let expected = Timestamp::from_second(i64::try_from(FROZEN_UNIX).unwrap()).unwrap();
        assert_eq!(timestamp, expected);
    }

    #[test]
    fn build_message_brackets_labels_only_when_present() {
        let labels = vec!["callgrind: 1 stored".to_owned()];
        let with_labels = build_message(false, 1, 1, &labels, &[]);
        assert!(
            with_labels.contains("[callgrind: 1 stored]"),
            "{with_labels}"
        );

        let without_labels = build_message(false, 0, 0, &[], &[]);
        assert!(!without_labels.contains('['), "{without_labels}");
    }

    #[test]
    fn build_message_reports_skipped_engines() {
        let stored = build_message(false, 0, 0, &[], &["criterion".to_owned()]);
        assert!(
            stored.contains("Skipped unsupported engine(s): criterion."),
            "{stored}"
        );

        let no_store = build_message(true, 0, 3, &[], &[]);
        assert!(
            no_store.contains("nothing stored (--no-store)"),
            "{no_store}"
        );
    }

    #[derive(Clone)]
    struct FakeRunner {
        status: EngineStatus,
        calls: Arc<Mutex<Vec<Vec<String>>>>,
    }

    impl FakeRunner {
        fn succeeding() -> Self {
            Self {
                status: EngineStatus {
                    success: true,
                    code: Some(0),
                },
                calls: Arc::default(),
            }
        }

        fn failing(code: i32) -> Self {
            Self {
                status: EngineStatus {
                    success: false,
                    code: Some(code),
                },
                calls: Arc::default(),
            }
        }

        fn last_command(&self) -> Option<Vec<String>> {
            self.calls.lock().unwrap().last().cloned()
        }
    }

    impl BenchRunner for FakeRunner {
        async fn run_engine(
            &self,
            argv: &[String],
            _env: &[(String, String)],
        ) -> io::Result<EngineStatus> {
            self.calls.lock().unwrap().push(argv.to_vec());
            Ok(self.status)
        }
    }

    #[derive(Clone)]
    struct FakeProbe {
        git: GitSnapshot,
        rustc: RustcInfo,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self {
                git: build_snapshot(
                    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    "deadbee",
                    "main",
                    "",
                    "",
                ),
                rustc: RustcInfo {
                    version: Some("1.91.0".to_owned()),
                    host: Some("x86_64-pc-windows-msvc".to_owned()),
                },
            }
        }
    }

    impl EnvironmentProbe for FakeProbe {
        async fn git(&self) -> io::Result<GitSnapshot> {
            Ok(self.git.clone())
        }

        async fn toolchain(&self) -> io::Result<RustcInfo> {
            Ok(self.rustc.clone())
        }
    }

    #[derive(Clone, Default)]
    struct FakeOutput {
        summaries: Vec<RawSummary>,
    }

    impl FakeOutput {
        fn with_two_callgrind_summaries() -> Self {
            Self {
                summaries: vec![
                    RawSummary {
                        path: PathBuf::from("a/summary.json"),
                        content: SINGLE_FIXTURE.to_owned(),
                    },
                    RawSummary {
                        path: PathBuf::from("b/summary.json"),
                        content: PARAMETRIZED_FIXTURE.to_owned(),
                    },
                ],
            }
        }

        fn with_malformed_summary() -> Self {
            Self {
                summaries: vec![RawSummary {
                    path: PathBuf::from("a/summary.json"),
                    content: "{ not valid json".to_owned(),
                }],
            }
        }
    }

    impl BenchOutputSource for FakeOutput {
        async fn collect(
            &self,
            _engine: EngineSystem,
            _since: SystemTime,
        ) -> io::Result<Vec<RawSummary>> {
            Ok(self.summaries.clone())
        }
    }

    fn config_with(engines: &str) -> Config {
        let text = format!("[storage.local]\npath = \"./data\"\n\n{engines}");
        parse_config(&text).expect("test configuration should parse")
    }

    fn callgrind_config() -> Config {
        config_with("[engines.callgrind]\ncommand = \"noop\"\n")
    }

    fn both_engines_config() -> Config {
        config_with(
            "[engines.callgrind]\ncommand = \"noop\"\n\n[engines.criterion]\ncommand = \"crit\"\n",
        )
    }

    fn drive(
        options: &RunOptions,
        config: &Config,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
    ) -> Result<RunOutcome, RunError> {
        let clock = Clock::new_frozen_at(frozen_time());
        let env = |_name: &str| None::<String>;
        let deps = RunDeps {
            runner,
            probe,
            output,
            storage,
            clock: &clock,
            env: &env,
            config,
            project_id: "folo",
            tool_version: "0.0.1",
            run_id: "test-run",
        };
        block_on(execute_run(options, &deps))
    }

    #[test]
    fn happy_path_stores_one_set_with_all_records() {
        let runner = FakeRunner::succeeding();
        let probe = FakeProbe::new();
        let output = FakeOutput::with_two_callgrind_summaries();
        let storage = MemoryStorage::new();

        let outcome = drive(
            &RunOptions::default(),
            &callgrind_config(),
            &runner,
            &probe,
            &output,
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 1"), "{message}");

        let keys = storage.keys();
        assert_eq!(
            keys,
            vec![
                "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
                 1700000000-deadbee-test-run.json"
                    .to_owned()
            ]
        );

        let bytes = block_on(storage.get(&keys[0])).unwrap();
        let set = ResultSet::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        assert_eq!(set.schema_version, crate::SCHEMA_VERSION);
        assert_eq!(set.results.len(), 2);
        assert_eq!(
            set.context.toolchain.target_triple,
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(set.context.toolchain.host_triple, "x86_64-pc-windows-msvc");
    }

    #[test]
    fn backfill_timestamp_names_the_object_by_override() {
        let options = RunOptions {
            timestamp: Some("2020-01-01T00:00:00Z".parse().unwrap()),
            ..RunOptions::default()
        };
        let storage = MemoryStorage::new();

        drive(
            &options,
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let keys = storage.keys();
        assert!(
            keys[0].contains("/1577836800-deadbee-test-run.json"),
            "{keys:?}"
        );
    }

    #[test]
    fn no_store_harvests_without_writing() {
        let options = RunOptions {
            no_store: true,
            ..RunOptions::default()
        };
        let storage = MemoryStorage::new();

        let outcome = drive(
            &options,
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("nothing stored"), "{message}");
        assert!(storage.keys().is_empty());
    }

    #[test]
    fn non_zero_engine_exit_is_an_error() {
        let storage = MemoryStorage::new();
        let error = drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::failing(101),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap_err();

        match error {
            RunError::Engine { engine, code } => {
                assert_eq!(engine, "callgrind");
                assert_eq!(code, Some(101));
            }
            other => panic!("expected engine error, got {other:?}"),
        }
        assert!(storage.keys().is_empty());
    }

    #[test]
    fn implicit_run_skips_unsupported_criterion() {
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Skipped"), "{message}");
        assert!(message.contains("criterion"), "{message}");
        assert_eq!(storage.keys().len(), 1);
    }

    #[test]
    fn explicit_criterion_is_rejected_as_unsupported() {
        let options = RunOptions {
            engine: Some("criterion".to_owned()),
            ..RunOptions::default()
        };
        let error = drive(
            &options,
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::default(),
            &MemoryStorage::new(),
        )
        .unwrap_err();

        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("not supported"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn passthrough_arguments_reach_the_runner_verbatim() {
        let runner = FakeRunner::succeeding();
        let options = RunOptions {
            passthrough: vec!["-p".to_owned(), "nm".to_owned()],
            ..RunOptions::default()
        };

        drive(
            &options,
            &callgrind_config(),
            &runner,
            &FakeProbe::new(),
            &FakeOutput::default(),
            &MemoryStorage::new(),
        )
        .unwrap();

        assert_eq!(
            runner
                .last_command()
                .expect("a command should have been recorded"),
            ["noop", "-p", "nm"]
        );
    }

    #[test]
    fn malformed_summary_is_a_parse_error() {
        let storage = MemoryStorage::new();
        let error = drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_malformed_summary(),
            &storage,
        )
        .unwrap_err();

        match error {
            RunError::Parse { message } => {
                assert!(message.contains("Callgrind"), "{message}");
            }
            other => panic!("expected parse error, got {other:?}"),
        }
        assert!(storage.keys().is_empty());
    }

    #[test]
    fn malformed_engine_command_is_a_command_error() {
        // The TOML value `prog "unterminated` has an unbalanced quote, so
        // tokenization fails before the engine is ever spawned.
        let config = config_with("[engines.callgrind]\ncommand = \"prog \\\"unterminated\"\n");
        let storage = MemoryStorage::new();
        let error = drive(
            &RunOptions::default(),
            &config,
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::default(),
            &storage,
        )
        .unwrap_err();

        match error {
            RunError::Command { engine, message } => {
                assert_eq!(engine, "callgrind");
                assert!(message.contains("valid shell"), "{message}");
            }
            other => panic!("expected command error, got {other:?}"),
        }
        assert!(storage.keys().is_empty());
    }

    #[test]
    fn resolve_engines_errors_when_none_configured() {
        let error = resolve_engines(&config_with(""), None).unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("no engines are configured"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_engines_rejects_unknown_config_engine() {
        let config = config_with("[engines.dhat]\ncommand = \"x\"\n");
        let error = resolve_engines(&config, None).unwrap_err();
        assert!(matches!(error, RunError::NoEngine { .. }));
    }

    #[test]
    fn resolve_engines_rejects_unknown_requested_engine() {
        let error = resolve_engines(&callgrind_config(), Some("dhat")).unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("unknown engine"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_engines_rejects_unconfigured_requested_engine() {
        let error = resolve_engines(&callgrind_config(), Some("criterion")).unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("not configured"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_engines_errors_when_only_unsupported_configured() {
        let config = config_with("[engines.criterion]\ncommand = \"crit\"\n");
        let error = resolve_engines(&config, None).unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("only unsupported"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn build_command_line_tokenizes_and_appends_verbatim() {
        assert_eq!(
            build_command_line("just bench-cg", &[], &["-p".to_owned(), "nm".to_owned()]).unwrap(),
            ["just", "bench-cg", "-p", "nm"]
        );
        assert_eq!(
            build_command_line("cargo bench", &["--quiet".to_owned()], &[]).unwrap(),
            ["cargo", "bench", "--quiet"]
        );
        // A quoted segment in the configured command is one argument, and a
        // forwarded argument with spaces is appended verbatim (not re-tokenized).
        assert_eq!(
            build_command_line("prog \"a b\"", &[], &["c d".to_owned()]).unwrap(),
            ["prog", "a b", "c d"]
        );
    }

    #[test]
    fn build_command_line_rejects_malformed_and_empty_commands() {
        let unbalanced = build_command_line("prog \"unterminated", &[], &[]).unwrap_err();
        assert!(unbalanced.contains("valid shell"), "{unbalanced}");
        let empty = build_command_line("   ", &[], &[]).unwrap_err();
        assert!(empty.contains("empty"), "{empty}");
    }
}
