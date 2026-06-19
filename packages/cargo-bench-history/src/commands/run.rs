//! The `run` command: execute the configured engines and store the results.
//!
//! Orchestration is generic over a set of small async ports (process runner,
//! environment probe, benchmark-output source, storage) plus an injected
//! [`tick::Clock`], so the whole flow is exercised in-process with fakes. The
//! public [`execute`] wires the real adapters and is what the binary runs.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::Timestamp;
use tick::Clock;

use crate::bench::{injected_bench_env, parse_callgrind_summary, parse_criterion_case};
use crate::bench_output::{BenchOutputSource, FsBenchOutputSource, Harvest};
use crate::comparability::{ComparabilityKey, EngineSystem, resolve_target_triple};
use crate::config::{StorageConfig, load_config};
use crate::context::{
    CiInfo, RunContext, Timestamps, ToolchainInfo, detect_ci, resolve_effective_time,
};
use crate::git::GitSnapshot;
use crate::host::RustcInfo;
use crate::machine::{HardwareProfile, resolve_machine_key};
use crate::model::{ResultRecord, ResultSet};
use crate::probe::{EnvironmentProbe, SystemProbe};
use crate::process::{BenchRunner, TokioBenchRunner};
use crate::report::{Reporter, StderrReporter};
use crate::storage::{Storage, StorageError, build_storage};
use crate::text::count_noun;
use crate::wiring::{default_config_path, resolve_project_id};
use crate::{RunError, RunOptions, RunOutcome};

/// The program and base arguments the production tool runs to benchmark the
/// workspace. The first-class scope flags (`--workspace`/`--package`/`--bench`)
/// and any `--` passthrough are appended to this.
const DEFAULT_BENCH_COMMAND: [&str; 2] = ["cargo", "bench"];

/// The label used for the single benchmark command in error messages.
const BENCH_COMMAND_LABEL: &str = "cargo bench";

/// The injected collaborators an orchestrated run operates against.
pub(crate) struct RunDeps<'a, R, P, O, S> {
    /// Launches the benchmark command.
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
    /// Resolved project identity for the storage partition.
    pub(crate) project_id: &'a str,
    /// Version of this tool, recorded with each run.
    pub(crate) tool_version: &'a str,
    /// Cargo target directory that engines write to and the harvest scans. It is
    /// injected into the benchmark command's environment as `CARGO_TARGET_DIR` so
    /// the output always lands where the harvest looks for it.
    pub(crate) target_root: &'a Path,
    /// The benchmark command (program plus base arguments) run once per `run`.
    /// Production uses `cargo bench`; the scope flags and passthrough are appended
    /// to it. Tests inject a mock program in its place.
    pub(crate) bench_command: &'a [String],
    /// Sink for `--verbose` diagnostic notes describing each step of the run.
    pub(crate) reporter: &'a dyn Reporter,
}

/// The real `run`: wire the production adapters and orchestrate.
///
/// `target_root` overrides the cargo target directory the harvest scans.
/// Production passes `None`, resolving the root from `CARGO_TARGET_DIR` (or the
/// `target/` default) as an absolute path. `bench_command` overrides the program
/// run to produce benchmark output; production passes `None`, defaulting to
/// `cargo bench`. Tests pass explicit values so the flow is hermetic without
/// mutating the process environment.
pub(crate) async fn execute(
    options: &RunOptions,
    target_root: Option<PathBuf>,
    bench_command: Option<Vec<String>>,
) -> Result<RunOutcome, RunError> {
    let reporter = StderrReporter::new(options.verbose);

    let config_path = options
        .config_path
        .clone()
        .unwrap_or_else(default_config_path);
    reporter.note(&format!(
        "loading configuration from {}",
        config_path.display()
    ));
    let config = load_config(&config_path).await?;

    let workspace_dir = std::env::current_dir().map_err(RunError::Io)?;
    let project_id = resolve_project_id(&config, &workspace_dir);
    reporter.note(&format!("project id: {project_id}"));
    reporter.note(&format!(
        "storage backend: {}",
        describe_storage(&config.storage)
    ));
    let storage = build_storage(&config)?;

    let runner = TokioBenchRunner::default();
    let probe = SystemProbe::default();
    let target_root = target_root.unwrap_or_else(resolve_target_root);
    reporter.note(&format!(
        "cargo target directory (scanned for engine output): {}",
        target_root.display()
    ));
    let output = FsBenchOutputSource::new(target_root.clone());
    let clock = Clock::new_tokio();
    let env = |name: &str| std::env::var(name).ok();
    let bench_command = bench_command.unwrap_or_else(default_bench_command);

    let deps = RunDeps {
        runner: &runner,
        probe: &probe,
        output: &output,
        storage: &storage,
        clock: &clock,
        env: &env,
        project_id: &project_id,
        tool_version: env!("CARGO_PKG_VERSION"),
        target_root: &target_root,
        bench_command: &bench_command,
        reporter: &reporter,
    };

    execute_run(options, &deps).await
}

/// A short human-readable description of where results are stored, for the
/// verbose diagnostic trail.
fn describe_storage(storage: &StorageConfig) -> String {
    match storage {
        StorageConfig::Local { path } => {
            format!("local filesystem at {}", path.display())
        }
        StorageConfig::Azure {
            account, container, ..
        } => format!("Azure Blob (account {account}, container {container})"),
    }
}

/// The production benchmark command: `cargo bench`.
pub(crate) fn default_bench_command() -> Vec<String> {
    DEFAULT_BENCH_COMMAND
        .iter()
        .map(|part| (*part).to_owned())
        .collect()
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

/// The result of harvesting one engine's output.
struct EngineSummary {
    /// Whether a result set was stored.
    stored: bool,
    /// Number of benchmark cases harvested.
    count: usize,
    /// Human-readable per-engine summary, or `None` when the engine produced no
    /// output (so it is silently absent from the summary).
    label: Option<String>,
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

/// Runs the benchmark command once and harvests every engine's output.
///
/// This is the storage-aware core shared by the `run` command and `backfill`:
/// the former wraps the summary in a human-readable message, the latter maps it
/// to a per-commit outcome. The benchmark command (`cargo bench` in production)
/// is run a single time with the union of every engine's injected environment;
/// each engine is then identified by which output tree it populated. An engine
/// that produced no output (for example Callgrind off Linux, where its benches
/// compile to no-ops) simply contributes nothing.
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
    let argv = build_bench_argv(deps.bench_command, options)?;

    // The benchmark command runs once with the union of every engine's injected
    // environment plus `CARGO_TARGET_DIR` pinned to the directory the harvest
    // scans, so engine output always lands where it is collected from — notably
    // when an ambient `CARGO_TARGET_DIR` (such as the one `cargo llvm-cov` sets)
    // differs from the root this run resolved.
    let mut env = injected_bench_env();
    env.push((
        "CARGO_TARGET_DIR".to_owned(),
        deps.target_root.to_string_lossy().into_owned(),
    ));

    if deps.reporter.enabled() {
        deps.reporter
            .note(&format!("running benchmark command: {}", argv.join(" ")));
        let rendered_env = env
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(", ");
        deps.reporter
            .note(&format!("injected environment: {rendered_env}"));
    }

    let run_start = deps.clock.system_time();
    let status = deps.runner.run_benches(&argv, &env).await?;
    if !status.success {
        return Err(RunError::Engine {
            engine: BENCH_COMMAND_LABEL.to_owned(),
            code: status.code,
        });
    }
    deps.reporter.note(&format!(
        "benchmark command finished; harvesting output modified at or after {} \
         (older files are treated as stale leftovers)",
        timestamp_from(run_start)
    ));

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
    let mut labels = Vec::new();

    for engine in EngineSystem::ALL {
        let summary = harvest_engine(options, deps, &shared, engine, run_start).await?;
        if summary.stored {
            stored = stored.saturating_add(1);
        }
        harvested = harvested.saturating_add(summary.count);
        if let Some(label) = summary.label {
            labels.push(label);
        }
    }

    Ok(RunSummary {
        stored,
        harvested,
        labels,
    })
}

/// Builds the benchmark command line: the base command followed by the cargo
/// scope flags translated from the run options, then any `--` passthrough.
///
/// Scope follows cargo's own conventions: with no `--package` filters the whole
/// workspace is benched (`--workspace`); otherwise each requested package is
/// passed with `--package`. Any `--bench` filters and passthrough arguments are
/// appended verbatim. Non-overlapping `--package`/`--bench` runs at one commit
/// therefore exercise disjoint benchmark cases.
fn build_bench_argv(
    bench_command: &[String],
    options: &RunOptions,
) -> Result<Vec<String>, RunError> {
    let Some((program, _)) = bench_command.split_first() else {
        return Err(RunError::Command {
            engine: BENCH_COMMAND_LABEL.to_owned(),
            message: "the benchmark command is empty".to_owned(),
        });
    };
    debug_assert!(!program.is_empty());

    let mut argv = bench_command.to_vec();
    if options.packages.is_empty() {
        argv.push("--workspace".to_owned());
    } else {
        for package in &options.packages {
            argv.push("--package".to_owned());
            argv.push(package.clone());
        }
    }
    for bench in &options.benches {
        argv.push("--bench".to_owned());
        argv.push(bench.clone());
    }
    argv.extend(options.passthrough.iter().cloned());
    Ok(argv)
}

/// Harvests one engine's output and (unless suppressed) stores the result set.
async fn harvest_engine<R, P, O, S>(
    options: &RunOptions,
    deps: &RunDeps<'_, R, P, O, S>,
    shared: &SharedContext,
    engine: EngineSystem,
    run_start: SystemTime,
) -> Result<EngineSummary, RunError>
where
    R: BenchRunner,
    P: EnvironmentProbe,
    O: BenchOutputSource,
    S: Storage,
{
    let harvest = deps
        .output
        .collect(engine, run_start, deps.reporter)
        .await?;
    let records = parse_harvest(&harvest)?;
    let count = records.len();

    // An engine that produced no fresh output contributes nothing. Off Linux the
    // Callgrind tree is simply absent; a `--package`/`--bench` filter may also
    // match no cases for one engine. Storing an empty result set would inflate
    // `analyze`'s run count with a series-less object, so skip it silently — there
    // is no misconfiguration to report, since absence is the expected steady state
    // for an engine that does not apply here.
    if count == 0 {
        deps.reporter.note(&format!(
            "{engine}: no fresh benchmark cases harvested; nothing to store"
        ));
        return Ok(EngineSummary {
            stored: false,
            count: 0,
            label: None,
        });
    }

    if options.no_store {
        deps.reporter.note(&format!(
            "{engine}: harvested {}; not storing (--no-store)",
            count_noun(count, "case")
        ));
        return Ok(EngineSummary {
            stored: false,
            count,
            label: Some(format!("{engine}: {count} harvested (not stored)")),
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
    // `--machine-key` overrides the computed fingerprint. Hardware-independent
    // engines (such as Callgrind) use no machine key.
    let machine_key = engine
        .is_hardware_dependent()
        .then(|| resolve_machine_key(options.machine_key.as_deref(), &shared.hardware));
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

    deps.reporter.note(&format!(
        "{engine}: {} at commit {commit} ({}), effective {effective}{} -> {object_key}",
        count_noun(count, "case"),
        if dirty { "dirty" } else { "clean" },
        machine_key
            .as_deref()
            .map_or_else(String::new, |key| format!(", machine {key}")),
    ));

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
    deps.reporter
        .note(&format!("{engine}: stored {object_key}"));

    Ok(EngineSummary {
        stored: true,
        count,
        label: Some(format!("{engine}: {count} stored")),
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

/// Builds the human-readable run summary.
fn build_message(no_store: bool, stored: usize, harvested: usize, labels: &[String]) -> String {
    let mut message = if no_store {
        format!(
            "Harvested {}; nothing stored (--no-store).",
            count_noun(harvested, "benchmark case")
        )
    } else {
        format!(
            "Stored {} covering {}.",
            count_noun(stored, "result set"),
            count_noun(harvested, "benchmark case")
        )
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

/// The cargo target directory, honoring `CARGO_TARGET_DIR`, as an absolute path.
fn resolve_target_root() -> PathBuf {
    let base = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    target_root_from(std::env::var_os("CARGO_TARGET_DIR"), &base)
}

/// Resolves the cargo target directory from an optional `CARGO_TARGET_DIR` value,
/// falling back to the conventional `target/` directory when it is unset, and
/// makes the result absolute by joining a relative path onto `base`.
///
/// Absolutizing matters because the value is injected back as `CARGO_TARGET_DIR`
/// for the benchmark command. Cargo runs each benchmark binary with its working
/// directory set to the owning package's directory, so a relative `target/` would
/// be resolved there by an engine such as Criterion — depositing output under the
/// package instead of the workspace root the harvest scans. An absolute root keeps
/// the write and the scan pointed at the same tree regardless of that cwd.
fn target_root_from(configured: Option<std::ffi::OsString>, base: &Path) -> PathBuf {
    let configured = configured.map_or_else(|| PathBuf::from("target"), PathBuf::from);
    if configured.is_absolute() {
        configured
    } else {
        base.join(configured)
    }
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
    use crate::git::build_snapshot;
    use crate::process::EngineStatus;
    use crate::report::RecordingReporter;
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
        let base = if cfg!(windows) {
            Path::new(r"C:\work")
        } else {
            Path::new("/work")
        };
        let resolved = target_root_from(None, base);
        assert_eq!(resolved, base.join("target"));
        assert!(resolved.is_absolute());
    }

    #[test]
    fn target_root_resolves_a_relative_cargo_target_dir_against_the_base() {
        let base = if cfg!(windows) {
            Path::new(r"C:\work")
        } else {
            Path::new("/work")
        };
        let resolved = target_root_from(Some(std::ffi::OsString::from("out")), base);
        assert_eq!(resolved, base.join("out"));
        assert!(resolved.is_absolute());
    }

    #[test]
    fn target_root_honors_an_explicit_cargo_target_dir() {
        let absolute = if cfg!(windows) {
            r"C:\custom\out"
        } else {
            "/custom/out"
        };
        let configured = std::ffi::OsString::from(absolute);
        assert_eq!(
            target_root_from(Some(configured), Path::new("ignored-base")),
            PathBuf::from(absolute)
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Reads the process working directory, which Miri cannot access.
    fn resolve_target_root_matches_the_ambient_environment() {
        // Resolving never panics, yields an absolute path, and agrees with
        // `target_root_from` for whatever `CARGO_TARGET_DIR` happens to be set
        // (or unset) in this process.
        let base = std::env::current_dir().expect("current directory is available");
        let resolved = resolve_target_root();
        assert_eq!(
            resolved,
            target_root_from(std::env::var_os("CARGO_TARGET_DIR"), &base)
        );
        assert!(resolved.is_absolute());
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

    #[test]
    fn build_message_pluralizes_counts_correctly() {
        let singular = build_message(false, 1, 1, &[]);
        assert!(
            singular.contains("Stored 1 result set covering 1 benchmark case."),
            "{singular}"
        );

        let plural = build_message(false, 2, 3, &[]);
        assert!(
            plural.contains("Stored 2 result sets covering 3 benchmark cases."),
            "{plural}"
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
        async fn run_benches(
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
        async fn collect(
            &self,
            engine: EngineSystem,
            _since: SystemTime,
            _reporter: &dyn Reporter,
        ) -> io::Result<Harvest> {
            Ok(match engine {
                EngineSystem::Callgrind => Harvest::Callgrind(self.callgrind.clone()),
                EngineSystem::Criterion => Harvest::Criterion(self.criterion.clone()),
            })
        }
    }

    /// The benchmark program tests pretend to run. The [`FakeRunner`] records the
    /// argv and never executes anything, so the value only needs to be non-empty.
    fn mock_bench_command() -> Vec<String> {
        vec!["mock".to_owned()]
    }

    fn drive(
        options: &RunOptions,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
    ) -> Result<RunOutcome, RunError> {
        drive_at(FROZEN_UNIX, options, runner, probe, output, storage)
    }

    fn drive_at(
        now_unix: u64,
        options: &RunOptions,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
    ) -> Result<RunOutcome, RunError> {
        let reporter = StderrReporter::new(false);
        drive_at_with(now_unix, options, runner, probe, output, storage, &reporter)
    }

    fn drive_at_with(
        now_unix: u64,
        options: &RunOptions,
        runner: &FakeRunner,
        probe: &FakeProbe,
        output: &FakeOutput,
        storage: &MemoryStorage,
        reporter: &dyn Reporter,
    ) -> Result<RunOutcome, RunError> {
        let now = SystemTime::UNIX_EPOCH
            .checked_add(Duration::from_secs(now_unix))
            .expect("frozen instant is within range");
        let clock = Clock::new_frozen_at(now);
        let env = |_name: &str| None::<String>;
        let bench_command = mock_bench_command();
        let deps = RunDeps {
            runner,
            probe,
            output,
            storage,
            clock: &clock,
            env: &env,
            project_id: "folo",
            tool_version: "0.0.1",
            target_root: Path::new("target"),
            bench_command: &bench_command,
            reporter,
        };
        block_on(execute_run(options, &deps))
    }

    #[test]
    fn verbose_run_notes_the_command_and_stored_key() {
        let runner = FakeRunner::succeeding();
        let probe = FakeProbe::new();
        let output = FakeOutput::with_two_callgrind_summaries();
        let storage = MemoryStorage::new();
        let reporter = RecordingReporter::new();

        drive_at_with(
            FROZEN_UNIX,
            &RunOptions::default(),
            &runner,
            &probe,
            &output,
            &storage,
            &reporter,
        )
        .unwrap();

        assert!(
            reporter.contains("running benchmark command: mock"),
            "expected an argv note, got {:?}",
            reporter.notes()
        );
        assert!(
            reporter.contains("GUNGRAUN_SAVE_SUMMARY"),
            "expected the injected-env note, got {:?}",
            reporter.notes()
        );
        assert!(
            reporter.contains("stored v2/folo/callgrind/"),
            "expected a stored-key note, got {:?}",
            reporter.notes()
        );
    }

    #[test]
    fn verbose_run_notes_an_empty_harvest() {
        let runner = FakeRunner::succeeding();
        let probe = FakeProbe::new();
        let output = FakeOutput::default();
        let storage = MemoryStorage::new();
        let reporter = RecordingReporter::new();

        drive_at_with(
            FROZEN_UNIX,
            &RunOptions::default(),
            &runner,
            &probe,
            &output,
            &storage,
            &reporter,
        )
        .unwrap();

        assert!(
            reporter.contains("no fresh benchmark cases harvested"),
            "expected an empty-harvest note, got {:?}",
            reporter.notes()
        );
        assert!(storage.keys().is_empty());
    }

    #[test]
    fn happy_path_stores_one_set_with_all_records() {
        let runner = FakeRunner::succeeding();
        let probe = FakeProbe::new();
        let output = FakeOutput::with_two_callgrind_summaries();
        let storage = MemoryStorage::new();

        let outcome = drive(&RunOptions::default(), &runner, &probe, &output, &storage).unwrap();

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
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let error = drive(
            &RunOptions::default(),
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
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();
        drive_at(
            FROZEN_UNIX + 1,
            &RunOptions::default(),
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
            &FakeRunner::succeeding(),
            &FakeProbe::dirty(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap();

        let error = drive_at(
            FROZEN_UNIX,
            &RunOptions::default(),
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
        // A benchmark command that exits cleanly but produces no fresh output (for
        // example a `--bench` filter that matched nothing, or an engine that does
        // not apply on this OS) must not store an empty result set, which would
        // otherwise inflate `analyze`'s run count.
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::default(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(message.contains("Stored 0 result sets"), "{message}");
        // An engine that produced nothing is absent from the summary rather than
        // flagged, because absence is the expected steady state off its OS.
        assert!(!message.contains('['), "{message}");
        assert!(storage.keys().is_empty(), "{:?}", storage.keys());
    }

    #[test]
    fn empty_harvest_for_one_engine_does_not_block_the_other() {
        // With only Criterion producing output, the empty Callgrind harvest is
        // skipped while Criterion is still stored.
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
            &FakeRunner::succeeding(),
            &FakeProbe::new(),
            &FakeOutput::with_criterion_case(),
            &storage,
        )
        .unwrap();

        let RunOutcome::Completed { message } = outcome else {
            panic!("expected completion");
        };
        assert!(
            message.contains("Stored 1 result set covering"),
            "{message}"
        );

        let keys = storage.keys();
        assert_eq!(keys.len(), 1, "{keys:?}");
        assert!(keys[0].contains("/criterion/"), "{keys:?}");
    }

    #[test]
    fn non_zero_engine_exit_is_an_error() {
        let storage = MemoryStorage::new();
        let error = drive(
            &RunOptions::default(),
            &FakeRunner::failing(101),
            &FakeProbe::new(),
            &FakeOutput::with_two_callgrind_summaries(),
            &storage,
        )
        .unwrap_err();

        match error {
            RunError::Engine { engine, code } => {
                assert_eq!(engine, "cargo bench");
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
    fn criterion_output_is_stored() {
        let storage = MemoryStorage::new();
        let outcome = drive(
            &RunOptions::default(),
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

        // Only Criterion produced output, partitioned by the machine fingerprint.
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
            machine_key: Some("ci-pool-a".to_owned()),
            ..RunOptions::default()
        };
        drive(
            &options,
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
        let error = drive(
            &RunOptions::default(),
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
            passthrough: vec!["--".to_owned(), "--quiet".to_owned()],
            ..RunOptions::default()
        };

        drive(
            &options,
            &runner,
            &FakeProbe::new(),
            &FakeOutput::default(),
            &MemoryStorage::new(),
        )
        .unwrap();

        // The benchmark command is the mock program, the default `--workspace`
        // scope, then the passthrough forwarded verbatim.
        assert_eq!(
            runner
                .last_command()
                .expect("a command should have been recorded"),
            ["mock", "--workspace", "--", "--quiet"]
        );
    }

    #[test]
    fn bench_environment_pins_the_target_directory() {
        let runner = FakeRunner::succeeding();

        drive(
            &RunOptions::default(),
            &runner,
            &FakeProbe::new(),
            &FakeOutput::default(),
            &MemoryStorage::new(),
        )
        .unwrap();

        let env = runner
            .last_env()
            .expect("an environment should have been recorded");
        // The benchmark command receives the combined engine environment (the
        // Callgrind summary flag) plus the resolved target directory, so output
        // lands exactly where the harvest scans.
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
    fn build_bench_argv_defaults_to_the_whole_workspace() {
        let argv = build_bench_argv(&mock_bench_command(), &RunOptions::default()).unwrap();
        assert_eq!(argv, ["mock", "--workspace"]);
    }

    #[test]
    fn build_bench_argv_translates_package_filters() {
        let options = RunOptions {
            packages: vec!["nm".to_owned(), "many_cpus".to_owned()],
            ..RunOptions::default()
        };
        let argv = build_bench_argv(&mock_bench_command(), &options).unwrap();
        // Packages omit `--workspace` and each becomes a `--package` pair.
        assert_eq!(argv, ["mock", "--package", "nm", "--package", "many_cpus"]);
    }

    #[test]
    fn build_bench_argv_translates_bench_filters_and_passthrough_in_order() {
        let options = RunOptions {
            packages: vec!["nm".to_owned()],
            benches: vec!["nm_observe".to_owned()],
            passthrough: vec!["--".to_owned(), "--noplot".to_owned()],
            ..RunOptions::default()
        };
        let argv = build_bench_argv(&mock_bench_command(), &options).unwrap();
        assert_eq!(
            argv,
            [
                "mock",
                "--package",
                "nm",
                "--bench",
                "nm_observe",
                "--",
                "--noplot"
            ]
        );
    }

    #[test]
    fn build_bench_argv_rejects_an_empty_command() {
        let error = build_bench_argv(&[], &RunOptions::default()).unwrap_err();
        match error {
            RunError::Command { engine, message } => {
                assert_eq!(engine, "cargo bench");
                assert!(message.contains("empty"), "{message}");
            }
            other => panic!("expected command error, got {other:?}"),
        }
    }
}
