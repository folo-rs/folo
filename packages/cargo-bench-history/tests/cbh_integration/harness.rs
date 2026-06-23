pub(crate) use std::cell::RefCell;
pub(crate) use std::collections::HashMap;
pub(crate) use std::path::{Path, PathBuf};
use std::sync::LazyLock;

pub(crate) use cargo_bench_history::{
    BenchmarkId, CiInfo, Cli, Command, GitInfo, Metric, MetricKind, Overrides, ResultRecord,
    ResultSet, RunContext, RunError, RunOutcome, SCHEMA_VERSION, Timestamps, ToolchainInfo,
    default_template, run, run_with_overrides,
};
pub(crate) use jiff::Timestamp;
pub(crate) use serial_test::serial;
pub(crate) use testing::CwdGuard;

/// The mock engine binary path, provided by Cargo for the auto-discovered binary
/// target. It writes Gungraun summary fixtures into the target tree and exits with
/// a chosen code, standing in for a real benchmark engine.
pub(crate) const MOCK_ENGINE: &str = env!("CARGO_BIN_EXE_cargo-bench-history-mock-engine");

/// The tool version recorded with each run. The integration test compiles within
/// the package, so its `CARGO_PKG_VERSION` matches the version `run` records.
pub(crate) const TOOL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// A process-global base repository template: a `master`-branch repo carrying the
/// volatile-directory excludes and one empty `root` commit, built once via the real
/// git commands and copied into each fresh workspace by [`Workspace::init_repo`].
/// Copying the tiny pruned `.git` tree is far cheaper than re-spawning `git init` +
/// `git commit` per repository, where process startup — compounded by real-time
/// antivirus on Windows — dominates the integration suite. Built lazily on first use
/// and kept alive for the whole test binary (its `TempDir` is never dropped, so the
/// source `.git` outlives every copy).
static BASE_TEMPLATE: LazyLock<tempfile::TempDir> = LazyLock::new(build_base_template);

/// Runs `git -C <root> <args>`, returning its captured output and asserting success.
///
/// Every invocation injects the committer identity plus throughput-oriented config:
/// `core.fsync=none`/`core.fsyncObjectFiles=false` skip the per-object disk flush (by
/// far the largest per-commit cost on Windows, where real-time antivirus compounds it)
/// and `gc.auto=0` keeps a background repack from firing mid-test. The identity is
/// supplied here rather than via separate `git config` subprocesses so a fresh
/// repository needs only `init`/`commit`, not extra process spawns. These settings are
/// safe for throwaway test repositories and unknown keys are ignored by older git, so
/// they are inert where unsupported. Shared by [`Workspace::git`] and the one-time
/// [`build_base_template`] so both speak to git identically.
fn run_git(root: &Path, args: &[&str]) -> std::process::Output {
    let root = root.to_string_lossy().into_owned();
    let mut full: Vec<&str> = vec![
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "user.name=Bench History Test",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "core.fsync=none",
        "-c",
        "core.fsyncObjectFiles=false",
        "-c",
        "gc.auto=0",
        "-C",
        root.as_str(),
    ];
    full.extend_from_slice(args);
    let output = std::process::Command::new("git")
        .args(&full)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// Builds the shared base template (see [`BASE_TEMPLATE`]) in its own temporary
/// directory using the same real git helpers a live repo uses, then prunes the
/// per-init noise git writes that only inflates each copy: the `*.sample` hook scripts
/// and the placeholder `description`.
fn build_base_template() -> tempfile::TempDir {
    let template = tempfile::tempdir().unwrap();
    let root = template.path();
    run_git(root, &["init", "-b", "master"]);
    std::fs::write(
        root.join(".git").join("info").join("exclude"),
        "/.cargo/\n/store/\n/target/\n",
    )
    .unwrap();
    run_git(root, &["commit", "--allow-empty", "-m", "root"]);
    let git_dir = root.join(".git");
    if let Ok(entries) = std::fs::read_dir(git_dir.join("hooks")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "sample") {
                remove_best_effort(&path);
            }
        }
    }
    remove_best_effort(&git_dir.join("description"));
    template
}

/// Removes `path`, ignoring failure. Pruning the template's per-init noise is purely
/// an optimization: a file that cannot be removed (already absent on some git version,
/// say) only leaves the copy marginally larger and is never an error.
fn remove_best_effort(path: &Path) {
    match std::fs::remove_file(path) {
        Ok(()) | Err(_) => {}
    }
}

/// Recursively copies the directory tree at `src` into `dst`, creating `dst` and any
/// missing parents. Used to clone the base template's `.git` into a fresh workspace.
fn copy_dir_all(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_all(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}
/// A hermetic workspace for driving `run` against the real adapters.
///
/// Writes a configuration and (optionally) fake engine output under a temporary
/// directory, then drives a command with that directory as the process current
/// directory, restoring the original directory before returning so assertions and
/// temp-dir cleanup happen outside it (required on Windows).
pub(crate) struct Workspace {
    dir: tempfile::TempDir,
    /// Maps a short commit label (for example `c1`) to the full SHA of the empty
    /// commit created for it, so seeded storage keys and the live git topology
    /// agree on the commit-directory segment. Interior mutability lets the
    /// `&self` seed helpers create commits lazily.
    commits: RefCell<HashMap<String, String>>,
    /// Arguments passed to the mock benchmark engine that `run`/`backfill` invoke
    /// in place of `cargo bench`. They tell the mock which fixtures to emit (which
    /// `--summary` / `--criterion` cases, or an `--exit-code`), so each engine's
    /// output tree is produced by the single benchmark command `run` invokes.
    bench: Vec<String>,
}

impl Workspace {
    /// Creates a workspace with `config` at the default `.cargo/bench_history.toml`.
    pub(crate) fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().unwrap(),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        };
        let cargo_dir = workspace.root().join(".cargo");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(cargo_dir.join("bench_history.toml"), config).unwrap();
        workspace
    }

    /// Creates a workspace with `config` plus an initialized git repository (on a
    /// `master` branch with one empty root commit), as `analyze` requires a
    /// repository to resolve a series' timeline from git topology.
    pub(crate) fn repo(config: &str) -> Self {
        let workspace = Self::new(config);
        workspace.init_repo();
        workspace
    }

    /// Alias for [`repo`](Self::repo): the standard repository already keeps its
    /// working tree clean across a `run` (the volatile `.cargo`/`store`/`target`
    /// directories are git-ignored). Retained for call sites that want to spell out
    /// that a clean tree — and therefore `history`-mode analysis — is intended.
    pub(crate) fn clean_repo(config: &str) -> Self {
        Self::repo(config)
    }

    /// Creates an empty workspace with no configuration file written.
    pub(crate) fn empty() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        }
    }

    /// Creates a workspace with `config` at a non-default `relative` path.
    pub(crate) fn with_config_at(relative: &str, config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().unwrap(),
            commits: RefCell::new(HashMap::new()),
            bench: Vec::new(),
        };
        let path = workspace.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, config).unwrap();
        workspace
    }

    /// Sets the arguments the mock benchmark engine receives, describing the
    /// fixtures it should emit (consuming builder used at construction). A run
    /// invokes the mock once with these arguments and harvests every engine's
    /// output tree it wrote.
    pub(crate) fn with_bench(mut self, args: &[&str]) -> Self {
        self.bench = args.iter().map(|arg| (*arg).to_owned()).collect();
        self
    }

    pub(crate) fn root(&self) -> &Path {
        self.dir.path()
    }

    /// Runs `git -C <root> <args>` against this workspace, returning its captured
    /// output. Thin wrapper over [`run_git`]; the repository directory is addressed
    /// explicitly so commit creation never depends on the process current directory.
    pub(crate) fn git(&self, args: &[&str]) -> std::process::Output {
        run_git(self.root(), args)
    }

    /// Initializes a `master`-branch repository with one empty root commit so `HEAD`
    /// always resolves. Rather than re-spawning `git init` + `git commit` for every
    /// repository (process startup, amplified by real-time antivirus, dominates these
    /// tests on Windows), it copies the pre-built [`BASE_TEMPLATE`] `.git` tree, which
    /// already carries the volatile-directory excludes (`.cargo`, `store`, `target`)
    /// and the empty root commit. The excludes are untracked, so the working tree stays
    /// clean unless a test deliberately dirties it (via [`make_dirty`]); a clean tree on
    /// the base branch is what makes `analyze` pick `history` mode.
    ///
    /// [`make_dirty`]: Self::make_dirty
    pub(crate) fn init_repo(&self) {
        copy_dir_all(
            &BASE_TEMPLATE.path().join(".git"),
            &self.root().join(".git"),
        );
        // Guard against a malformed template (for example a loose ref the copy
        // dropped): resolving HEAD straight from the copied `.git` must yield a full
        // SHA, so a broken copy fails loudly here instead of misbehaving in a later
        // command. The read is cheap and never spawns git for a healthy template.
        assert_eq!(
            self.head_sha().len(),
            40,
            "copied base template did not yield a resolvable HEAD"
        );
    }

    /// Reads `HEAD`'s commit SHA straight from the `.git` directory, avoiding a
    /// `git rev-parse` subprocess (process startup dominates these tests on
    /// Windows). For the small, freshly-created repositories the harness builds the
    /// branch ref is always loose; a `git rev-parse` fallback covers the unexpected
    /// (packed refs, detached `HEAD`).
    pub(crate) fn head_sha(&self) -> String {
        let git_dir = self.root().join(".git");
        if let Ok(head) = std::fs::read_to_string(git_dir.join("HEAD")) {
            let head = head.trim();
            if let Some(reference) = head.strip_prefix("ref: ") {
                // `refs/heads/<branch>` uses `/`, which the Windows path APIs accept.
                if let Ok(sha) = std::fs::read_to_string(git_dir.join(reference)) {
                    let sha = sha.trim();
                    if sha.len() == 40 {
                        return sha.to_owned();
                    }
                }
            } else if head.len() == 40 {
                return head.to_owned();
            }
        }
        let head = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(head.stdout).unwrap().trim().to_owned()
    }

    /// Lazily creates an empty commit labeled `label` on the current branch and
    /// returns its full SHA, reusing the SHA if the label was already created.
    ///
    /// Creation order is the first-parent topology order, so seeding in oldest-first
    /// order makes the storage keys line up with the live git history `analyze`
    /// reconstructs the series from.
    pub(crate) fn commit(&self, label: &str) -> String {
        if let Some(sha) = self.commits.borrow().get(label) {
            return sha.clone();
        }
        self.git(&["commit", "--allow-empty", "-m", label]);
        let sha = self.head_sha();
        self.commits
            .borrow_mut()
            .insert(label.to_owned(), sha.clone());
        sha
    }

    /// Creates and checks out a new branch off the current `HEAD`.
    pub(crate) fn checkout_new_branch(&self, name: &str) {
        self.git(&["checkout", "-b", name]);
    }

    /// Checks out an existing branch.
    pub(crate) fn checkout(&self, name: &str) {
        self.git(&["checkout", name]);
    }

    /// Merges `branch` into the current branch with an always-materialized merge
    /// commit (`--no-ff`), labels the resulting merge commit `label`, and returns
    /// its full SHA. The merge commit has the current branch's tip as its first
    /// parent and `branch`'s tip as its second, so it sits on the current branch's
    /// first-parent line while the merged-in commits stay off it — the topology the
    /// git-aware `analyze`/`backfill` selection must respect.
    pub(crate) fn merge(&self, branch: &str, label: &str) -> String {
        self.git(&["merge", "--no-ff", "--no-edit", "-m", label, branch]);
        let sha = self.head();
        self.commits
            .borrow_mut()
            .insert(label.to_owned(), sha.clone());
        sha
    }

    /// Commits a tracked file with `contents` at `relative` on the current branch
    /// and returns the new commit's full SHA. Unlike [`commit`](Self::commit), the
    /// commit changes the tree, so the file appears in any worktree checked out to
    /// it — used to stand in for a "broken" commit the mock engine reacts to.
    pub(crate) fn commit_with_file(&self, message: &str, relative: &str, contents: &str) -> String {
        std::fs::write(self.root().join(relative), contents).unwrap();
        self.git(&["add", relative]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    /// Removes a previously tracked file at `relative`, commits the removal on the
    /// current branch, and returns the new commit's full SHA.
    pub(crate) fn commit_removing_file(&self, message: &str, relative: &str) -> String {
        self.git(&["rm", relative]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    /// The full SHA of the current `HEAD`.
    pub(crate) fn head(&self) -> String {
        let output = self.git(&["rev-parse", "HEAD"]);
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// The name of the currently checked-out branch.
    pub(crate) fn current_branch(&self) -> String {
        let output = self.git(&["rev-parse", "--abbrev-ref", "HEAD"]);
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    /// Writes an untracked file at `relative`, leaving the working tree dirty (it
    /// is neither committed nor git-ignored).
    pub(crate) fn make_dirty(&self, relative: &str) {
        std::fs::write(self.root().join(relative), "uncommitted\n").unwrap();
    }

    /// Reads a file relative to the workspace root, if it exists.
    pub(crate) fn read(&self, relative: &str) -> Option<String> {
        std::fs::read_to_string(self.root().join(relative)).ok()
    }

    /// Drives a command with `args` against this workspace.
    pub(crate) async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        // Point the harvest at this workspace's own `target/` explicitly, so a
        // shared ambient `CARGO_TARGET_DIR` (as `cargo llvm-cov` sets during
        // coverage runs) cannot make the harvester pick up summaries written by
        // other tests sharing that directory. Passing the root through the API
        // keeps the test hermetic without mutating the process environment.
        let target_root = self.root().join("target");

        // Drive `run`/`backfill` against the mock engine instead of `cargo bench`:
        // the program plus its fixture-describing arguments form the benchmark
        // command, which the single bench invocation runs to produce engine output.
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: Some(target_root),
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }

    /// Drives a command with an explicit benchmark command, overriding the
    /// workspace's configured one for this invocation only. This lets a test make
    /// the engine behave differently across two drives — for example, succeed on a
    /// first backfill, then exit non-zero if it is invoked at all on a re-run.
    pub(crate) async fn drive_with_bench(
        &self,
        bench: &[&str],
        args: &[&str],
    ) -> Result<RunOutcome, RunError> {
        let target_root = self.root().join("target");
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(bench.iter().map(|arg| (*arg).to_owned()));

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: Some(target_root),
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }
    ///
    /// [`drive`]: Self::drive
    pub(crate) async fn drive_resolving_target_root(
        &self,
        args: &[&str],
    ) -> Result<RunOutcome, RunError> {
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());

        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.root().to_path_buf()),
                target_root: None,
                bench_command: Some(bench_command),
                now: Some(analysis_now()),
            },
        )
        .await
    }

    /// All stored objects as `(object key, parsed result set)` pairs, sorted by key.
    pub(crate) fn stored_objects(&self) -> Vec<(String, ResultSet)> {
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
                let set = ResultSet::from_json(&std::fs::read_to_string(&path).unwrap()).unwrap();
                (key, set)
            })
            .collect();
        objects.sort_by(|left, right| left.0.cmp(&right.0));
        objects
    }

    /// Returns the single stored object, failing if there is not exactly one.
    pub(crate) fn single_object(&self) -> (String, ResultSet) {
        let mut objects = self.stored_objects();
        assert_eq!(
            objects.len(),
            1,
            "expected exactly one stored object, found {}",
            objects.len()
        );
        objects.pop().unwrap()
    }

    /// Writes `set` to `key` (a `/`-separated object key) under the local store,
    /// mirroring the layout `run` produces.
    pub(crate) fn seed(&self, key: &str, set: &ResultSet) {
        let mut path = self.root().join("store");
        for part in key.split('/') {
            path.push(part);
        }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, set.to_json().unwrap()).unwrap();
    }

    /// Seeds one Callgrind result set with an `Ir` value at the given `date`
    /// (`YYYY-MM-DD`, taken at UTC midnight) and commit `label`, creating the
    /// corresponding empty git commit so the storage key and live topology agree.
    pub(crate) fn seed_callgrind(&self, date: &str, label: &str, value: f64) {
        self.seed_callgrind_in("x86_64-unknown-linux-gnu", "synthetic", date, label, value);
    }

    /// Seeds one clean Callgrind result set into the `triple`/`machine` partition
    /// for commit `label`, so a single commit can host objects in several
    /// comparable sets (as parallel CI pools produce).
    pub(crate) fn seed_callgrind_in(
        &self,
        triple: &str,
        machine: &str,
        date: &str,
        label: &str,
        value: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!("v2/testproj/callgrind/{triple}/{machine}/{sha}/clean.json");
        self.seed(&key, &ir_result_set(effective.as_second(), &sha, value));
    }

    /// Seeds one *dirty* (uncommitted-tree) Callgrind snapshot with an `Ir` value
    /// at commit `label`, keyed by the effective second like a real dirty run.
    pub(crate) fn seed_dirty_callgrind(&self, date: &str, label: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/dirty-{}.json",
            effective.as_second()
        );
        self.seed(&key, &ir_result_set(effective.as_second(), &sha, value));
    }

    /// Seeds one Callgrind result set carrying `metrics` for benchmark
    /// `nm::observe/pull` at the given `date` (`YYYY-MM-DD`, UTC midnight) and
    /// commit `label`.
    pub(crate) fn seed_metrics(&self, date: &str, label: &str, metrics: Vec<Metric>) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
        self.seed(&key, &result_set_with(effective.as_second(), &sha, metrics));
    }

    /// Seeds a flat history followed by a clear, sustained upward step — a
    /// regression with enough points on each side to satisfy the change-point
    /// detector's persistence requirement.
    pub(crate) fn seed_rising_callgrind_history(&self) {
        self.seed_callgrind("2024-01-01", "c1", 100.0);
        self.seed_callgrind("2024-01-02", "c2", 100.0);
        self.seed_callgrind("2024-01-03", "c3", 100.0);
        self.seed_callgrind("2024-01-04", "c4", 130.0);
        self.seed_callgrind("2024-01-05", "c5", 130.0);
        self.seed_callgrind("2024-01-06", "c6", 130.0);
    }

    /// Seeds one Criterion `wall_time` result set at the given `date` (`YYYY-MM-DD`,
    /// UTC midnight), commit `label`, and machine-key partition `machine`.
    pub(crate) fn seed_criterion(&self, date: &str, label: &str, machine: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/criterion/x86_64-pc-windows-msvc/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &criterion_result_set(effective.as_second(), &sha, value),
        );
    }

    /// Seeds one *dirty* (uncommitted-tree) Criterion `wall_time` snapshot at the
    /// given `date`, commit `label`, and machine-key partition `machine`, keyed by
    /// the effective second like a real dirty run.
    pub(crate) fn seed_dirty_criterion(&self, date: &str, label: &str, machine: &str, value: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/criterion/x86_64-pc-windows-msvc/{machine}/{sha}/dirty-{}.json",
            effective.as_second()
        );
        self.seed(
            &key,
            &criterion_result_set(effective.as_second(), &sha, value),
        );
    }

    /// Seeds a flat Criterion `wall_time` history then a clear, sustained upward
    /// step. Four points on each side give the rank-sum gate enough power to
    /// distinguish the step from noise.
    pub(crate) fn seed_rising_criterion_history(&self, machine: &str) {
        self.seed_criterion("2024-02-01", "d1", machine, 20.0);
        self.seed_criterion("2024-02-02", "d2", machine, 20.0);
        self.seed_criterion("2024-02-03", "d3", machine, 20.0);
        self.seed_criterion("2024-02-04", "d4", machine, 20.0);
        self.seed_criterion("2024-02-05", "d5", machine, 30.0);
        self.seed_criterion("2024-02-06", "d6", machine, 30.0);
        self.seed_criterion("2024-02-07", "d7", machine, 30.0);
        self.seed_criterion("2024-02-08", "d8", machine, 30.0);
    }

    /// Seeds one Callgrind result set carrying two distinct benchmark identities,
    /// `alpha::bench/wide` and `beta::bench/narrow`, each with an `Ir` metric at the
    /// given value, stamped at `date` (`YYYY-MM-DD`, UTC midnight) and commit `label`.
    pub(crate) fn seed_two_benchmarks(&self, date: &str, label: &str, alpha: f64, beta: f64) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/callgrind/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json");
        self.seed(
            &key,
            &two_benchmark_result_set(effective.as_second(), &sha, alpha, beta),
        );
    }

    /// Seeds one clean `alloc_tracker` result set for `operation` at the given
    /// `date` and commit `label`, recording `bytes` mean bytes and `allocs` mean
    /// allocations per iteration. Allocation counts are deterministic, so the
    /// partition is `synthetic` (no machine key).
    pub(crate) fn seed_alloc_tracker(
        &self,
        date: &str,
        label: &str,
        operation: &str,
        bytes: f64,
        allocs: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key = format!(
            "v2/testproj/alloc_tracker/x86_64-unknown-linux-gnu/synthetic/{sha}/clean.json"
        );
        self.seed(
            &key,
            &alloc_result_set(effective.as_second(), &sha, operation, bytes, allocs),
        );
    }

    /// Seeds one clean `all_the_time` result set for `operation` at the given
    /// `date` and commit `label`, recording `nanos` mean processor-time nanoseconds
    /// per iteration in the `machine`-keyed partition (processor time is
    /// hardware-dependent).
    pub(crate) fn seed_all_the_time(
        &self,
        date: &str,
        label: &str,
        machine: &str,
        operation: &str,
        nanos: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/all_the_time/x86_64-unknown-linux-gnu/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &time_result_set(effective.as_second(), &sha, operation, nanos),
        );
    }

    /// Seeds one clean `all_the_time` result set for `operation`, like
    /// [`Self::seed_all_the_time`], but additionally recording a confidence
    /// interval of `nanos` ± `half_width` so the noise detector's
    /// CI-non-overlap gate has dispersion to compare.
    pub(crate) fn seed_all_the_time_with_interval(
        &self,
        date: &str,
        label: &str,
        machine: &str,
        operation: &str,
        nanos: f64,
        half_width: f64,
    ) {
        let sha = self.commit(label);
        let effective: Timestamp = format!("{date}T00:00:00Z").parse().unwrap();
        let key =
            format!("v2/testproj/all_the_time/x86_64-unknown-linux-gnu/{machine}/{sha}/clean.json");
        self.seed(
            &key,
            &time_result_set_with_dispersion(
                effective.as_second(),
                &sha,
                operation,
                nanos,
                half_width,
            ),
        );
    }
}

/// A fixed clock anchor for `analyze`/`list`'s history-mode default `--since`
/// window. Seed data lands across 2024; anchoring "now" at 2024-06-01 keeps the
/// default six-month look-back (cutoff 2023-12-01) inclusive of that data, so the
/// default window does not silently drop seeded points as wall-clock time passes.
pub(crate) fn analysis_now() -> Timestamp {
    "2024-06-01T00:00:00Z".parse::<Timestamp>().unwrap()
}

pub(crate) fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .unwrap()
        .into_command()
}

/// Builds a [`GitInfo`] for a committed run: the full hash is `<commit>full`, the
/// short hash is `commit`, and the branch is `main` (clean working tree).
pub(crate) fn git_info(commit: &str) -> GitInfo {
    GitInfo {
        commit: Some(format!("{commit}full")),
        short_commit: Some(commit.to_owned()),
        branch: Some("main".to_owned()),
        dirty: false,
    }
}

/// Builds a Callgrind result set with two records — `alpha::bench/wide` and
/// `beta::bench/narrow` — each carrying a single `Ir` metric, stamped with the
/// effective second and abbreviated `commit`.
pub(crate) fn two_benchmark_result_set(
    effective: i64,
    commit: &str,
    alpha: f64,
    beta: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let ir = |value: f64| {
        vec![Metric::new(
            "Ir".to_owned(),
            MetricKind::InstructionCount,
            value,
            Some("count".to_owned()),
        )]
    };
    let records = vec![
        ResultRecord::new(
            BenchmarkId::new(
                Some("alpha".to_owned()),
                "alpha::bench".to_owned(),
                Some("wide".to_owned()),
                None,
            ),
            ir(alpha),
        ),
        ResultRecord::new(
            BenchmarkId::new(
                Some("beta".to_owned()),
                "beta::bench".to_owned(),
                Some("narrow".to_owned()),
                None,
            ),
            ir(beta),
        ),
    ];
    ResultSet::new(context, records)
}

/// Builds a Criterion result set for benchmark `time/capture/std_instant`
/// carrying a single `wall_time` metric at `value` nanoseconds, stamped with the
/// effective second and abbreviated `commit`.
pub(crate) fn criterion_result_set(effective: i64, commit: &str, value: f64) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(
            None,
            "time/capture".to_owned(),
            Some("std_instant".to_owned()),
            None,
        ),
        vec![Metric::new(
            "wall_time".to_owned(),
            MetricKind::WallTime,
            value,
            Some("ns".to_owned()),
        )],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `alloc_tracker` result set for `operation` carrying an
/// `allocation_bytes` and an `allocation_count` metric, stamped with the effective
/// second and abbreviated `commit`.
pub(crate) fn alloc_result_set(
    effective: i64,
    commit: &str,
    operation: &str,
    bytes: f64,
    allocs: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![
            Metric::new(
                "allocated_bytes".to_owned(),
                MetricKind::AllocationBytes,
                bytes,
                Some("bytes".to_owned()),
            ),
            Metric::new(
                "allocations".to_owned(),
                MetricKind::AllocationCount,
                allocs,
                Some("count".to_owned()),
            ),
        ],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `all_the_time` result set for `operation` carrying a single
/// `processor_time` metric at `value` nanoseconds, stamped with the effective
/// second and abbreviated `commit`.
pub(crate) fn time_result_set(
    effective: i64,
    commit: &str,
    operation: &str,
    value: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![Metric::new(
            "processor_time".to_owned(),
            MetricKind::ProcessorTime,
            value,
            Some("ns".to_owned()),
        )],
    );
    ResultSet::new(context, vec![record])
}

/// Builds an `all_the_time` result set for `operation` carrying a single
/// `processor_time` metric at `value` nanoseconds, like [`time_result_set`],
/// but additionally recording a confidence interval of `value` ± `half_width`
/// (and a matching standard deviation) so the noise detector can apply its
/// CI-non-overlap gate to processor time.
pub(crate) fn time_result_set_with_dispersion(
    effective: i64,
    commit: &str,
    operation: &str,
    value: f64,
    half_width: f64,
) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(None, operation.to_owned(), None, None),
        vec![
            Metric::new(
                "processor_time".to_owned(),
                MetricKind::ProcessorTime,
                value,
                Some("ns".to_owned()),
            )
            .with_dispersion(
                Some(half_width),
                Some(value - half_width),
                Some(value + half_width),
            ),
        ],
    );
    ResultSet::new(context, vec![record])
}

pub(crate) fn ir_of(record: &ResultRecord) -> f64 {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == "Ir")
        .unwrap()
        .value
}

/// The metric named `name` within a record.
pub(crate) fn metric_named<'a>(record: &'a ResultRecord, name: &str) -> &'a Metric {
    record
        .metrics
        .iter()
        .find(|metric| metric.name == name)
        .unwrap()
}

/// Escapes a string for embedding in a TOML basic (double-quoted) string, so a
/// project id with unusual characters survives as a literal.
pub(crate) fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A configuration with local storage but no engines, used by `analyze` tests
/// that seed history directly and never launch an engine.
pub(crate) fn storage_only_config() -> String {
    "[project]\n\
     id = \"testproj\"\n\n\
     [storage.local]\n\
     path = \"./store\"\n"
        .to_owned()
}

/// Builds a Callgrind result set for benchmark `nm::observe/pull` carrying the
/// given `metrics`, stamped with the effective second and abbreviated `commit`.
pub(crate) fn result_set_with(effective: i64, commit: &str, metrics: Vec<Metric>) -> ResultSet {
    let time = Timestamp::from_second(effective).unwrap();
    let git = git_info(commit);
    let context = RunContext::new(
        Timestamps::new(time, time),
        git,
        CiInfo::default(),
        ToolchainInfo::default(),
        TOOL_VERSION.to_owned(),
    );
    let record = ResultRecord::new(
        BenchmarkId::new(
            Some("nm".to_owned()),
            "nm::observe".to_owned(),
            Some("pull".to_owned()),
            None,
        ),
        metrics,
    );
    ResultSet::new(context, vec![record])
}

/// Builds a Callgrind result set with a single `Ir` metric at `value`, stamped
/// with the given effective second and abbreviated `commit`.
pub(crate) fn ir_result_set(effective: i64, commit: &str, value: f64) -> ResultSet {
    result_set_with(
        effective,
        commit,
        vec![Metric::new(
            "Ir".to_owned(),
            MetricKind::InstructionCount,
            value,
            Some("count".to_owned()),
        )],
    )
}

/// A configuration with local storage under an explicit project `id` (which may
/// contain characters that require sanitizing for the storage partition).
pub(crate) fn storage_only_config_with_id(id: &str) -> String {
    format!(
        "[project]\n\
         id = \"{}\"\n\n\
         [storage.local]\n\
         path = \"./store\"\n",
        toml_escape(id)
    )
}

pub(crate) fn collect_json_files(dir: &Path, out: &mut Vec<PathBuf>) {
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
