//! The `run` command: execute the configured engines and store the results.
//!
//! Orchestration is generic over a set of small async ports (process runner,
//! environment probe, benchmark-output source, storage) plus an injected
//! [`tick::Clock`], so the whole flow is exercised in-process with fakes. The
//! public [`execute`] wires the real adapters and is what the binary runs.

use std::env::consts;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::Timestamp;
use tick::Clock;

use crate::bench::{injected_env, parse_callgrind_summary, parse_criterion_case};
use crate::bench_output::{BenchOutputSource, FsBenchOutputSource, Harvest};
use crate::comparability::{ComparabilityKey, EngineSystem, resolve_target_triple};
use crate::config::{Config, load_config};
use crate::context::{
    CiInfo, RunContext, Timestamps, ToolchainInfo, detect_ci, resolve_effective_time,
};
use crate::git::GitSnapshot;
use crate::host::RustcInfo;
use crate::machine::{HardwareProfile, resolve_machine_key};
use crate::model::{ResultRecord, ResultSet};
use crate::probe::{EnvironmentProbe, SystemProbe};
use crate::process::{BenchRunner, TokioBenchRunner};
use crate::storage::{Storage, StorageError, build_storage};
use crate::wiring::{default_config_path, resolve_project_id};
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
    /// Cargo target directory that engines write to and the harvest scans. It is
    /// injected into each engine's environment as `CARGO_TARGET_DIR` so the
    /// engine's output always lands where the harvest looks for it.
    pub(crate) target_root: &'a Path,
    /// The host operating system (Rust's `std::env::consts::OS` name) used to
    /// decide which engines run by default. An engine whose configured `os` list
    /// excludes this host is skipped unless explicitly requested with `--engine`.
    pub(crate) host_os: &'a str,
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
    let storage = build_storage(&config)?;

    let runner = TokioBenchRunner::default();
    let probe = SystemProbe::default();
    let target_root = target_root.unwrap_or_else(resolve_target_root);
    let output = FsBenchOutputSource::new(target_root.clone());
    let clock = Clock::new_tokio();
    let env = |name: &str| std::env::var(name).ok();

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
        target_root: &target_root,
        host_os: consts::OS,
    };

    execute_run(options, &deps).await
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
    /// Host hardware profile, fingerprinted for hardware-dependent engines.
    hardware: HardwareProfile,
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

/// Aggregate outcome of running every selected engine in one run.
pub(crate) struct RunSummary {
    /// Number of result sets stored.
    pub(crate) stored: usize,
    /// Number of benchmark cases harvested across all engines.
    pub(crate) harvested: usize,
    /// Per-engine human-readable labels.
    pub(crate) labels: Vec<String>,
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
    let summary = run_engines(options, deps).await?;
    let message = build_message(
        options.no_store,
        summary.stored,
        summary.harvested,
        &summary.labels,
    );
    Ok(RunOutcome::Completed { message })
}

/// Runs every selected engine and returns the aggregate [`RunSummary`].
///
/// This is the storage-aware core shared by the `run` command and `backfill`:
/// the former wraps the summary in a human-readable message, the latter maps it
/// to a per-commit outcome.
pub(crate) async fn run_engines<R, P, O, S>(
    options: &RunOptions,
    deps: &RunDeps<'_, R, P, O, S>,
) -> Result<RunSummary, RunError>
where
    R: BenchRunner,
    P: EnvironmentProbe,
    O: BenchOutputSource,
    S: Storage,
{
    let (to_run, skipped) = resolve_engines(deps.config, options.engine.as_deref(), deps.host_os)?;

    let rustc = deps.probe.toolchain().await?;
    let shared = SharedContext {
        git: deps.probe.git().await?,
        host_triple: rustc.host.clone().unwrap_or_default(),
        rustc,
        ci: detect_ci(deps.env),
        hardware: deps.probe.hardware().await,
    };

    let mut stored = 0_usize;
    let mut harvested = 0_usize;
    let mut labels = skipped;

    for (name, engine) in &to_run {
        let summary = process_engine(options, deps, &shared, name, *engine).await?;
        if summary.stored {
            stored = stored.saturating_add(1);
        }
        harvested = harvested.saturating_add(summary.count);
        labels.push(summary.label);
    }

    Ok(RunSummary {
        stored,
        harvested,
        labels,
    })
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
    let mut injected = injected_env(engine);
    // Pin the engine's output location to the directory the harvest scans, so the
    // two never diverge — notably when an ambient `CARGO_TARGET_DIR` (such as the
    // one `cargo llvm-cov` sets) differs from the root this run resolved.
    injected.push((
        "CARGO_TARGET_DIR".to_owned(),
        deps.target_root.to_string_lossy().into_owned(),
    ));
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

    let harvest = deps.output.collect(engine, run_start).await?;
    let records = parse_harvest(&harvest)?;
    let count = records.len();

    if options.no_store {
        return Ok(EngineSummary {
            stored: false,
            count,
            label: format!("{name}: {count} harvested (not stored)"),
        });
    }

    // An engine that harvested nothing produces no comparable data point. Storing an
    // empty result set would inflate `analyze`'s run count with a series-less object
    // (and silently mask a misconfigured filter or output path), so skip storage and
    // surface the empty harvest in the summary instead.
    if count == 0 {
        return Ok(EngineSummary {
            stored: false,
            count: 0,
            label: format!("{name}: no benchmark cases harvested (nothing stored)"),
        });
    }

    let execution = timestamp_from(run_start);
    let ingest: Timestamp = deps.clock.system_time_as();
    // A clean run records the committed code, so its effective time defaults to the
    // commit's committer date. A dirty snapshot does not correspond to any commit,
    // so it defaults to the wall clock instead. An explicit `--timestamp` overrides
    // either default (notably for backfilling historical data points).
    let dirty = shared.git.info.dirty;
    let committer_default = if dirty { None } else { shared.git.committer };
    let effective = resolve_effective_time(options.timestamp, committer_default, ingest);
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

    // Hardware-dependent engines (such as Criterion) partition their history by a
    // machine fingerprint so only equivalent machines share a series. An explicit
    // `--machine-key` (or its config equivalent) overrides the computed fingerprint.
    // Hardware-independent engines (such as Callgrind) use no machine key.
    let machine_key = engine.is_hardware_dependent().then(|| {
        resolve_machine_key(
            options
                .machine_key
                .as_deref()
                .or(deps.config.machine.key.as_deref()),
            &shared.hardware,
        )
    });
    let key = ComparabilityKey::new(
        deps.project_id,
        engine,
        &target_triple,
        machine_key.as_deref(),
    );
    // History is organized by commit, so the full commit SHA names the directory
    // (`analyze` resolves which commits to read from git topology). A clean run is
    // keyed solely by its commit and so is deterministic; a dirty snapshot adds its
    // effective time so concurrent snapshots of the same commit coexist.
    let commit = shared.git.info.commit.as_deref().unwrap_or("unknown");
    let object_key = if dirty {
        key.dirty_key(commit, effective.as_second())
    } else {
        key.clean_key(commit)
    };

    // A freshly built result set is composed of plain structs and finite counts,
    // so serialization cannot fail.
    let json = result_set
        .to_json()
        .expect("a freshly built result set always serializes to JSON");
    store_result(
        deps.storage,
        &object_key,
        json.as_bytes(),
        options.overwrite,
    )
    .await?;

    Ok(EngineSummary {
        stored: true,
        count,
        label: format!("{name}: {count} stored"),
    })
}

/// Persists a serialized result set at `object_key`.
///
/// A normal run is write-once: if an object already exists at the key (a clean
/// re-run of the same commit, or a dirty snapshot sharing an effective second),
/// the collision surfaces as [`RunError::Duplicate`] so the caller can refuse it.
/// An overwrite replaces any existing object in place instead.
async fn store_result<S: Storage>(
    storage: &S,
    object_key: &str,
    bytes: &[u8],
    overwrite: bool,
) -> Result<(), RunError> {
    if overwrite {
        storage.put_overwrite(object_key, bytes).await?;
        return Ok(());
    }
    match storage.put(object_key, bytes).await {
        Ok(()) => Ok(()),
        Err(StorageError::AlreadyExists { key }) => Err(RunError::Duplicate { key }),
        Err(error) => Err(error.into()),
    }
}

/// Parses harvested engine output into result records, naming the offending
/// source on a parse failure.
fn parse_harvest(harvest: &Harvest) -> Result<Vec<ResultRecord>, RunError> {
    match harvest {
        Harvest::Callgrind(summaries) => {
            let mut records = Vec::with_capacity(summaries.len());
            for summary in summaries {
                let record =
                    parse_callgrind_summary(&summary.content).map_err(|error| RunError::Parse {
                        message: format!("{}: {error}", summary.path.display()),
                    })?;
                records.push(record);
            }
            Ok(records)
        }
        Harvest::Criterion(cases) => {
            let mut records = Vec::with_capacity(cases.len());
            for case in cases {
                let record =
                    parse_criterion_case(&case.benchmark, &case.estimates).map_err(|error| {
                        RunError::Parse {
                            message: format!("{}: {error}", case.dir.display()),
                        }
                    })?;
                records.push(record);
            }
            Ok(records)
        }
    }
}

/// The engines selected to run, paired with human-readable notes about engines
/// skipped because their `os` restriction excludes the host.
type EngineSelection = (Vec<(String, EngineSystem)>, Vec<String>);

/// Decides which engines to run from the configuration and an optional filter.
///
/// Returns the engines to run as `(config key, engine)` pairs together with notes
/// describing engines skipped because their configured `os` list excludes
/// `host_os`. An explicit `--engine` filter must name a configured engine and
/// always overrides the host-OS restriction (so e.g. Callgrind can be forced
/// through WSL on Windows).
fn resolve_engines(
    config: &Config,
    requested: Option<&str>,
    host_os: &str,
) -> Result<EngineSelection, RunError> {
    let mut configured = Vec::new();
    for (name, engine_config) in &config.engines {
        let engine = EngineSystem::from_name(name).ok_or_else(|| RunError::NoEngine {
            message: format!("configuration references unknown engine {name:?}"),
        })?;
        configured.push((name.clone(), engine, engine_config.supports_host(host_os)));
    }

    let mut skipped = Vec::new();

    let to_run = match requested {
        Some(requested) => {
            let engine = EngineSystem::from_name(requested).ok_or_else(|| RunError::NoEngine {
                message: format!("unknown engine {requested:?}"),
            })?;
            if !configured
                .iter()
                .any(|(_, candidate, _)| *candidate == engine)
            {
                return Err(RunError::NoEngine {
                    message: format!("engine {requested:?} is not configured"),
                });
            }
            configured
                .into_iter()
                .filter(|(_, candidate, _)| *candidate == engine)
                .map(|(name, candidate, _)| (name, candidate))
                .collect()
        }
        None => {
            let mut to_run = Vec::new();
            for (name, engine, supported) in configured {
                if supported {
                    to_run.push((name, engine));
                } else {
                    skipped.push(format!(
                        "skipped {name} (not supported on {host_os}; force with --engine {name})"
                    ));
                }
            }
            to_run
        }
    };

    if to_run.is_empty() {
        let message = if skipped.is_empty() {
            "no engines are configured".to_owned()
        } else {
            format!("no engines run on {host_os}; force one with --engine <name>")
        };
        return Err(RunError::NoEngine { message });
    }

    Ok((to_run, skipped))
}
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
fn build_message(no_store: bool, stored: usize, harvested: usize, labels: &[String]) -> String {
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

    use std::io;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use futures::executor::block_on;

    use super::*;
    use crate::bench_output::{Harvest, RawCriterionCase, RawSummary};
    use crate::config::parse_config;
    use crate::git::build_snapshot;
    use crate::process::EngineStatus;
    use crate::storage::MemoryStorage;

    const SINGLE_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/single_unparametrized.summary.json");
    const PARAMETRIZED_FIXTURE: &str =
        include_str!("../../tests/fixtures/callgrind/parametrized.summary.json");
    const CRITERION_BENCHMARK_FIXTURE: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/benchmark.json");
    const CRITERION_ESTIMATES_FIXTURE: &str =
        include_str!("../../tests/fixtures/criterion/std_instant/estimates.json");

    /// The frozen wall-clock instant used by orchestration tests (2023-11-14Z).
    const FROZEN_UNIX: u64 = 1_700_000_000;

    fn frozen_time() -> SystemTime {
        SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(FROZEN_UNIX))
            .expect("frozen instant is within range")
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
        let with_labels = build_message(false, 1, 1, &labels);
        assert!(
            with_labels.contains("[callgrind: 1 stored]"),
            "{with_labels}"
        );

        let without_labels = build_message(false, 0, 0, &[]);
        assert!(!without_labels.contains('['), "{without_labels}");
    }

    #[test]
    fn build_message_reports_no_store() {
        let no_store = build_message(true, 0, 3, &[]);
        assert!(
            no_store.contains("nothing stored (--no-store)"),
            "{no_store}"
        );
    }

    /// One engine invocation's injected environment, recorded by [`FakeRunner`].
    type RecordedEnv = Vec<(String, String)>;

    #[derive(Clone)]
    struct FakeRunner {
        status: EngineStatus,
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        envs: Arc<Mutex<Vec<RecordedEnv>>>,
    }

    impl FakeRunner {
        fn succeeding() -> Self {
            Self {
                status: EngineStatus {
                    success: true,
                    code: Some(0),
                },
                calls: Arc::default(),
                envs: Arc::default(),
            }
        }

        fn failing(code: i32) -> Self {
            Self {
                status: EngineStatus {
                    success: false,
                    code: Some(code),
                },
                calls: Arc::default(),
                envs: Arc::default(),
            }
        }

        fn last_command(&self) -> Option<Vec<String>> {
            self.calls.lock().unwrap().last().cloned()
        }

        fn last_env(&self) -> Option<Vec<(String, String)>> {
            self.envs.lock().unwrap().last().cloned()
        }
    }

    impl BenchRunner for FakeRunner {
        async fn run_engine(
            &self,
            argv: &[String],
            env: &[(String, String)],
        ) -> io::Result<EngineStatus> {
            self.calls.lock().unwrap().push(argv.to_vec());
            self.envs.lock().unwrap().push(env.to_vec());
            Ok(self.status)
        }
    }

    #[derive(Clone)]
    struct FakeProbe {
        git: GitSnapshot,
        rustc: RustcInfo,
        hardware: HardwareProfile,
    }

    impl FakeProbe {
        fn new() -> Self {
            Self::with_status("")
        }

        /// A probe whose git snapshot reports a dirty working tree.
        fn dirty() -> Self {
            Self::with_status(" M src/lib.rs")
        }

        fn with_status(status: &str) -> Self {
            Self {
                git: build_snapshot(
                    "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                    "deadbee",
                    "main",
                    status,
                    "",
                ),
                rustc: RustcInfo {
                    version: Some("1.91.0".to_owned()),
                    host: Some("x86_64-pc-windows-msvc".to_owned()),
                },
                hardware: HardwareProfile {
                    processors: 8,
                    memory_regions: 1,
                    cpu_brand: Some("Test CPU 3000".to_owned()),
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

        async fn hardware(&self) -> HardwareProfile {
            self.hardware.clone()
        }
    }

    #[derive(Clone, Default)]
    struct FakeOutput {
        callgrind: Vec<RawSummary>,
        criterion: Vec<RawCriterionCase>,
    }

    impl FakeOutput {
        fn with_two_callgrind_summaries() -> Self {
            Self {
                callgrind: vec![
                    RawSummary {
                        path: PathBuf::from("a/summary.json"),
                        content: SINGLE_FIXTURE.to_owned(),
                    },
                    RawSummary {
                        path: PathBuf::from("b/summary.json"),
                        content: PARAMETRIZED_FIXTURE.to_owned(),
                    },
                ],
                ..Self::default()
            }
        }

        fn with_malformed_summary() -> Self {
            Self {
                callgrind: vec![RawSummary {
                    path: PathBuf::from("a/summary.json"),
                    content: "{ not valid json".to_owned(),
                }],
                ..Self::default()
            }
        }

        fn with_criterion_case() -> Self {
            Self {
                criterion: vec![RawCriterionCase {
                    dir: PathBuf::from("criterion/grp/std/now/new"),
                    benchmark: CRITERION_BENCHMARK_FIXTURE.to_owned(),
                    estimates: CRITERION_ESTIMATES_FIXTURE.to_owned(),
                }],
                ..Self::default()
            }
        }

        fn with_callgrind_and_criterion() -> Self {
            let mut output = Self::with_two_callgrind_summaries();
            output.criterion = Self::with_criterion_case().criterion;
            output
        }

        fn with_malformed_criterion_case() -> Self {
            Self {
                criterion: vec![RawCriterionCase {
                    dir: PathBuf::from("criterion/grp/std/now/new"),
                    benchmark: "{ not valid json".to_owned(),
                    estimates: CRITERION_ESTIMATES_FIXTURE.to_owned(),
                }],
                ..Self::default()
            }
        }
    }

    impl BenchOutputSource for FakeOutput {
        async fn collect(&self, engine: EngineSystem, _since: SystemTime) -> io::Result<Harvest> {
            Ok(match engine {
                EngineSystem::Callgrind => Harvest::Callgrind(self.callgrind.clone()),
                EngineSystem::Criterion => Harvest::Criterion(self.criterion.clone()),
            })
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
        drive_at(FROZEN_UNIX, options, config, runner, probe, output, storage)
    }

    fn drive_at(
        now_unix: u64,
        options: &RunOptions,
        config: &Config,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
    ) -> Result<RunOutcome, RunError> {
        drive_on_host(
            "linux", now_unix, options, config, runner, probe, output, storage,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "test helper threads every injected port"
    )]
    fn drive_on_host(
        host_os: &str,
        now_unix: u64,
        options: &RunOptions,
        config: &Config,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
    ) -> Result<RunOutcome, RunError> {
        let now = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(now_unix))
            .expect("frozen instant is within range");
        let clock = Clock::new_frozen_at(now);
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
            target_root: Path::new("target"),
            host_os,
        };
        block_on(execute_run(options, &deps))
    }

    #[test]
    fn os_restricted_engine_is_skipped_and_noted_on_unsupported_host() {
        // callgrind is restricted to linux; on a windows host the default run
        // skips it (reporting the skip) and still runs criterion.
        let config = config_with(
            "[engines.callgrind]\ncommand = \"noop\"\nos = [\"linux\"]\n\n[engines.criterion]\ncommand = \"crit\"\n",
        );
        let storage = MemoryStorage::new();

        let outcome = drive_on_host(
            "windows",
            FROZEN_UNIX,
            &RunOptions::default(),
            &config,
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_criterion_case(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("skipped callgrind"), "{message}");
        assert!(message.contains("windows"), "{message}");
        assert!(message.contains("--engine callgrind"), "{message}");

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "only criterion should store: {keys:?}");
        assert!(keys[0].contains("/criterion/"), "{keys:?}");
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
                "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
                 deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json"
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
    fn backfill_timestamp_records_the_override_as_the_effective_time() {
        // Clean keys no longer embed the effective time, so a backfill override is
        // verified by the stored effective timestamp rather than by the object key.
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
        assert!(keys[0].ends_with("/clean.json"), "{keys:?}");

        let bytes = block_on(storage.get(&keys[0])).unwrap();
        let set = ResultSet::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        let expected: Timestamp = "2020-01-01T00:00:00Z".parse().unwrap();
        assert_eq!(set.context.timestamps.effective, expected);
    }

    #[test]
    fn clean_re_run_of_the_same_commit_is_refused_as_a_duplicate() {
        let storage = MemoryStorage::new();
        drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let error = drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap_err();

        let RunError::Duplicate { key } = error else {
            panic!("expected a duplicate error, got {error:?}");
        };
        assert!(key.ends_with("/clean.json"), "{key}");
        // The second run left the single stored object untouched.
        assert_eq!(storage.keys().len(), 1);
    }

    #[test]
    fn overwrite_replaces_a_clean_result_in_place() {
        let storage = MemoryStorage::new();
        drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            // First run harvests two records.
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let overwrite = RunOptions {
            overwrite: true,
            ..RunOptions::default()
        };
        drive(
            &overwrite,
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            // Second run harvests a single record over the same key.
            &FakeOutput {
                callgrind: vec![RawSummary {
                    path: PathBuf::from("a/summary.json"),
                    content: SINGLE_FIXTURE.to_owned(),
                }],
                ..FakeOutput::default()
            },
            &storage,
        )
        .unwrap();

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(keys[0].ends_with("/clean.json"), "{keys:?}");
        let bytes = block_on(storage.get(&keys[0])).unwrap();
        let set = ResultSet::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        assert_eq!(
            set.results.len(),
            1,
            "overwrite should replace the contents"
        );
    }

    #[test]
    fn dirty_run_is_keyed_by_effective_time() {
        let storage = MemoryStorage::new();
        drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(
            keys[0].ends_with(&format!(
                "/deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/dirty-{FROZEN_UNIX}.json"
            )),
            "{keys:?}"
        );
    }

    #[test]
    fn two_dirty_runs_at_different_times_coexist() {
        let storage = MemoryStorage::new();
        drive_at(
            FROZEN_UNIX,
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();
        drive_at(
            FROZEN_UNIX + 1,
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let keys = storage.keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
    }

    #[test]
    fn dirty_runs_sharing_an_effective_second_collide() {
        // Two dirty snapshots taken at the same effective second map to the same
        // key, so the second is refused as a duplicate just like a clean re-run.
        let storage = MemoryStorage::new();
        drive_at(
            FROZEN_UNIX,
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let error = drive_at(
            FROZEN_UNIX,
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap_err();

        assert!(matches!(error, RunError::Duplicate { .. }), "{error:?}");
        assert_eq!(storage.keys().len(), 1);

        // With --overwrite the clash is resolved by replacing the object in place.
        let overwrite = RunOptions {
            overwrite: true,
            ..RunOptions::default()
        };
        drive_at(
            FROZEN_UNIX,
            &overwrite,
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();
        assert_eq!(storage.keys().len(), 1);
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
    fn empty_harvest_stores_nothing_and_reports_it() {
        // An engine that exits cleanly but produces no fresh output (e.g. a
        // benchmark-name filter that matched nothing) must not store an empty
        // result set, which would otherwise inflate `analyze`'s run count.
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &callgrind_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::default(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 0 result set(s)"), "{message}");
        assert!(
            message.contains("no benchmark cases harvested (nothing stored)"),
            "{message}"
        );
        assert!(storage.keys().is_empty(), "{:?}", storage.keys());
    }

    #[test]
    fn empty_harvest_for_one_engine_does_not_block_the_other() {
        // With both engines configured but only Criterion producing output, the
        // empty Callgrind harvest is skipped while Criterion is still stored.
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_criterion_case(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 1 result set(s)"), "{message}");

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(keys[0].contains("/criterion/"), "{keys:?}");
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
    fn both_engines_run_stores_a_set_per_engine() {
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_callgrind_and_criterion(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 2"), "{message}");

        // Callgrind partitions under `synthetic`; Criterion partitions under the
        // machine fingerprint of the probed hardware. Both sets are stored.
        let keys = storage.keys();
        assert_eq!(keys.len(), 2, "{keys:?}");
        assert!(
            keys.iter()
                .any(|key| key.contains("/callgrind/") && key.contains("/synthetic/")),
            "{keys:?}"
        );
        assert!(
            keys.iter()
                .any(|key| key.contains("/criterion/") && !key.contains("/synthetic/")),
            "{keys:?}"
        );
    }

    #[test]
    fn explicit_criterion_selection_stores_results() {
        let storage = MemoryStorage::new();
        let options = RunOptions {
            engine: Some("criterion".to_owned()),
            ..RunOptions::default()
        };
        let outcome = drive(
            &options,
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_criterion_case(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 1"), "{message}");

        // Only the criterion engine ran, partitioned by the machine fingerprint.
        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(keys[0].contains("/criterion/"), "{keys:?}");

        let bytes = block_on(storage.get(&keys[0])).unwrap();
        let set = ResultSet::from_json(&String::from_utf8(bytes).unwrap()).unwrap();
        assert_eq!(set.results.len(), 1);
        assert_eq!(set.results[0].metrics[0].kind, crate::MetricKind::WallTime);
    }

    #[test]
    fn criterion_partition_uses_the_machine_key_override() {
        let storage = MemoryStorage::new();
        let options = RunOptions {
            engine: Some("criterion".to_owned()),
            machine_key: Some("ci-pool-a".to_owned()),
            ..RunOptions::default()
        };
        drive(
            &options,
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_criterion_case(),
            &storage,
        )
        .unwrap();

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(
            keys[0].contains("/criterion/x86_64-pc-windows-msvc/ci-pool-a/"),
            "{keys:?}"
        );
    }

    #[test]
    fn malformed_criterion_case_is_a_parse_error() {
        let storage = MemoryStorage::new();
        let options = RunOptions {
            engine: Some("criterion".to_owned()),
            ..RunOptions::default()
        };
        let error = drive(
            &options,
            &both_engines_config(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_malformed_criterion_case(),
            &storage,
        )
        .unwrap_err();

        match error {
            RunError::Parse { message } => {
                assert!(message.contains("Criterion"), "{message}");
                // The offending case directory is named so failures are actionable.
                assert!(message.contains("new"), "{message}");
            }
            other => panic!("expected parse error, got {other:?}"),
        }
        assert!(storage.keys().is_empty());
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
    fn engine_environment_pins_the_target_directory() {
        let runner = FakeRunner::succeeding();

        drive(
            &RunOptions::default(),
            &callgrind_config(),
            &runner,
            &FakeProbe::new(),
            &FakeOutput::default(),
            &MemoryStorage::new(),
        )
        .unwrap();

        let env = runner
            .last_env()
            .expect("an environment should have been recorded");
        // The engine receives the callgrind summary flag plus the resolved target
        // directory, so its output lands exactly where the harvest scans.
        assert!(
            env.contains(&("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())),
            "{env:?}"
        );
        assert!(
            env.contains(&("CARGO_TARGET_DIR".to_owned(), "target".to_owned())),
            "{env:?}"
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
                // The offending file is named so multi-summary failures are
                // actionable.
                assert!(message.contains("summary.json"), "{message}");
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
        let error = resolve_engines(&config_with(""), None, "linux").unwrap_err();
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
        let error = resolve_engines(&config, None, "linux").unwrap_err();
        assert!(matches!(error, RunError::NoEngine { .. }));
    }

    #[test]
    fn resolve_engines_rejects_unknown_requested_engine() {
        let error = resolve_engines(&callgrind_config(), Some("dhat"), "linux").unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("unknown engine"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_engines_rejects_unconfigured_requested_engine() {
        let error = resolve_engines(&callgrind_config(), Some("criterion"), "linux").unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("not configured"), "{message}");
            }
            other => panic!("expected no-engine error, got {other:?}"),
        }
    }

    #[test]
    fn resolve_engines_accepts_a_criterion_only_configuration() {
        let config = config_with("[engines.criterion]\ncommand = \"crit\"\n");
        let (to_run, skipped) = resolve_engines(&config, None, "linux").unwrap();
        assert_eq!(to_run.len(), 1);
        assert_eq!(to_run[0].0, "criterion");
        assert_eq!(to_run[0].1, EngineSystem::Criterion);
        assert!(skipped.is_empty());
    }

    #[test]
    fn resolve_engines_skips_os_restricted_engine_by_default() {
        let config = config_with(
            "[engines.callgrind]\ncommand = \"noop\"\nos = [\"linux\"]\n\n[engines.criterion]\ncommand = \"crit\"\n",
        );
        let (to_run, skipped) = resolve_engines(&config, None, "windows").unwrap();
        assert_eq!(to_run.len(), 1);
        assert_eq!(to_run[0].1, EngineSystem::Criterion);
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].contains("callgrind"), "{}", skipped[0]);
        assert!(skipped[0].contains("windows"), "{}", skipped[0]);
    }

    #[test]
    fn resolve_engines_runs_os_restricted_engine_on_supported_host() {
        let config = config_with("[engines.callgrind]\ncommand = \"noop\"\nos = [\"linux\"]\n");
        let (to_run, skipped) = resolve_engines(&config, None, "linux").unwrap();
        assert_eq!(to_run.len(), 1);
        assert_eq!(to_run[0].1, EngineSystem::Callgrind);
        assert!(skipped.is_empty());
    }

    #[test]
    fn resolve_engines_forces_os_restricted_engine_when_requested() {
        let config = config_with("[engines.callgrind]\ncommand = \"noop\"\nos = [\"linux\"]\n");
        let (to_run, skipped) = resolve_engines(&config, Some("callgrind"), "windows").unwrap();
        assert_eq!(to_run.len(), 1);
        assert_eq!(to_run[0].1, EngineSystem::Callgrind);
        assert!(skipped.is_empty());
    }

    #[test]
    fn resolve_engines_errors_when_only_engine_is_os_restricted() {
        let config = config_with("[engines.callgrind]\ncommand = \"noop\"\nos = [\"linux\"]\n");
        let error = resolve_engines(&config, None, "windows").unwrap_err();
        match error {
            RunError::NoEngine { message } => {
                assert!(message.contains("no engines run on windows"), "{message}");
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
