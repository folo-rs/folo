//! The process port: launching engine commands (streaming their output) and
//! capturing the output of helper commands such as `git` and `rustc`.
//!
//! The real adapter uses `tokio::process`; an in-memory fake (in `#[cfg(test)]`)
//! substitutes for it so orchestration is testable without spawning processes.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

/// Launches the benchmark command (`cargo bench`) with injected environment
/// variables.
pub trait BenchRunner {
    /// Runs `argv` directly (no shell) with `env` applied, letting the child
    /// inherit stdio so benchmark progress streams to the terminal. `argv[0]` is
    /// the program and the remainder are its arguments, each passed verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be spawned or awaited.
    fn run_benches(
        &self,
        argv: &[String],
        env: &[(String, String)],
    ) -> impl Future<Output = io::Result<EngineStatus>>;
}

/// The exit outcome of an engine command, in a portable shape fakes can build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineStatus {
    /// Whether the process reported success.
    pub success: bool,
    /// The process exit code, if one was reported.
    pub code: Option<i32>,
}

impl EngineStatus {
    /// Captures the portable outcome of a finished process.
    fn from_exit(status: ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

/// The captured result of a helper command.
#[derive(Clone, Debug)]
pub struct CommandOutput {
    /// The process exit status.
    pub status: ExitStatus,
    /// Captured standard output, lossily decoded as UTF-8.
    pub stdout: String,
}

/// The real [`BenchRunner`], backed by `tokio::process`.
///
/// By default the engine command runs in the process working directory.
/// [`in_dir`](Self::in_dir) runs it in a specific directory that is still this
/// tool's own workspace; [`in_worktree`](Self::in_worktree) runs it in a
/// historical checkout, which additionally governs its own toolchain.
#[derive(Clone, Debug, Default)]
pub struct TokioBenchRunner {
    /// Working directory for the engine command; the process CWD when absent.
    dir: Option<PathBuf>,
    /// Whether the launcher's toolchain selection is removed from the child
    /// environment. It is kept for the tool's own workspace, where the caller
    /// picked that toolchain deliberately (`cargo +nightly run`), and removed for
    /// a historical checkout, which must be built by the toolchain it pins itself.
    detach_from_launcher_toolchain: bool,
}

impl TokioBenchRunner {
    /// Runs the engine command in `dir` instead of the process CWD.
    ///
    /// `dir` is treated as this tool's own workspace, so the toolchain the tool
    /// was launched with also builds the benchmarks.
    #[must_use]
    pub fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            detach_from_launcher_toolchain: false,
        }
    }

    /// Runs the engine command in `dir`, a **historical checkout** rather than
    /// this tool's own workspace.
    ///
    /// Behaves like [`in_dir`](Self::in_dir) but additionally detaches the child
    /// from the launcher's toolchain selection (see
    /// [`detach_from_launcher_toolchain`]), so the benchmarks of a past commit are
    /// built by the toolchain that commit pins rather than by whichever toolchain
    /// happened to build this tool.
    #[must_use]
    pub fn in_worktree(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
            detach_from_launcher_toolchain: true,
        }
    }

    /// Builds the configured engine command without launching it.
    ///
    /// Split from [`run_benches`](BenchRunner::run_benches) so the working
    /// directory and environment this runner imposes are asserted without spawning
    /// a process.
    fn command(&self, argv: &[String], env: &[(String, String)]) -> tokio::process::Command {
        let mut command = engine_command(argv);

        if let Some(dir) = self.dir.as_deref() {
            command.current_dir(dir);
        }
        if self.detach_from_launcher_toolchain {
            detach_from_launcher_toolchain(&mut command);
        }

        // Applied last so an explicitly injected value always wins over the
        // detachment above.
        for (name, value) in env {
            command.env(name, value);
        }

        command
    }
}

impl BenchRunner for TokioBenchRunner {
    async fn run_benches(
        &self,
        argv: &[String],
        env: &[(String, String)],
    ) -> io::Result<EngineStatus> {
        Ok(EngineStatus::from_exit(
            self.command(argv, env).status().await?,
        ))
    }
}

/// The environment variables through which a rustup-proxied launcher imposes its
/// own toolchain on everything it spawns.
///
/// `cargo run` exports both: `RUSTUP_TOOLCHAIN` names the toolchain that built
/// this tool and **overrides** any `rust-toolchain.toml` in the directory a child
/// `cargo`/`rustc` runs in, and `CARGO` is the absolute path to that toolchain's
/// `cargo` binary.
const LAUNCHER_TOOLCHAIN_VARS: [&str; 2] = ["RUSTUP_TOOLCHAIN", "CARGO"];

/// Removes the launcher's toolchain selection from `command`'s environment, so the
/// rustup proxy resolves the toolchain from the directory the command runs in.
///
/// Nothing else is stripped: `CARGO_HOME`/`RUSTUP_HOME` still locate the rustup
/// installation (which auto-installs a pinned toolchain it does not have yet), and
/// `RUSTFLAGS` is caller intent that the tool passes through by contract.
fn detach_from_launcher_toolchain(command: &mut tokio::process::Command) {
    for name in LAUNCHER_TOOLCHAIN_VARS {
        command.env_remove(name);
    }
}

/// Runs `program` with `args`, capturing its standard output.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or awaited.
pub async fn capture(program: &str, args: &[&str]) -> io::Result<CommandOutput> {
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    capture_output(command).await
}

/// Runs `program` with `args` **inside a historical checkout** at `dir`,
/// capturing its standard output.
///
/// Mirrors [`TokioBenchRunner::in_worktree`] for helper commands: the command runs
/// in `dir` and is detached from the launcher's toolchain selection (see
/// [`detach_from_launcher_toolchain`]), so a `rustc` queried this way describes the
/// same compiler that builds that checkout's benchmarks.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or awaited.
pub async fn capture_in_worktree(
    program: &str,
    args: &[&str],
    dir: &Path,
) -> io::Result<CommandOutput> {
    let mut command = tokio::process::Command::new(program);
    command.args(args).current_dir(dir);
    detach_from_launcher_toolchain(&mut command);
    capture_output(command).await
}

/// Awaits `command` to completion, capturing its standard output.
async fn capture_output(mut command: tokio::process::Command) -> io::Result<CommandOutput> {
    let output = command.stdin(Stdio::null()).output().await?;

    Ok(CommandOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
    })
}

/// Builds a process command that runs `argv` directly, without a shell.
///
/// `argv[0]` is the program and the remainder are its arguments, each passed
/// verbatim — no shell tokenization, quoting, or metacharacter interpretation,
/// so forwarded arguments containing spaces or quotes survive intact.
fn engine_command(argv: &[String]) -> tokio::process::Command {
    let (program, args) = argv
        .split_first()
        .expect("engine argv is non-empty; build_bench_argv rejects empty commands");
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::{LAUNCHER_TOOLCHAIN_VARS, TokioBenchRunner, engine_command};

    /// The environment overrides a runner applies to its child, keyed by variable
    /// name. A `None` value is a removal, which is how the launcher's toolchain
    /// selection is kept out of a historical checkout's build.
    fn env_overrides(runner: &TokioBenchRunner) -> HashMap<OsString, Option<OsString>> {
        let argv = vec!["cargo".to_owned(), "bench".to_owned()];
        runner
            .command(&argv, &[])
            .as_std()
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
            .collect()
    }

    #[test]
    fn in_dir_sets_the_working_directory() {
        // `in_dir` must record the target directory; the default runner leaves it
        // absent (engine runs in the process CWD), so the two must differ.
        let runner = TokioBenchRunner::in_dir("some/worktree");
        assert_eq!(runner.dir, Some(PathBuf::from("some/worktree")));
        assert_eq!(TokioBenchRunner::default().dir, None);
    }

    #[test]
    fn in_worktree_runs_in_the_directory_like_in_dir() {
        let runner = TokioBenchRunner::in_worktree("some/worktree");
        assert_eq!(runner.dir, Some(PathBuf::from("some/worktree")));
    }

    #[test]
    // The Windows environment map compares keys with `CompareStringOrdinal`, a
    // foreign function Miri cannot call; the same assertion runs under Linux Miri.
    #[cfg_attr(all(miri, windows), ignore)]
    fn in_worktree_detaches_the_child_from_the_launcher_toolchain() {
        // A historical checkout must be built by the toolchain it pins, so the
        // selection `cargo run` exported into this process is removed from the
        // child (a removal surfaces as a `None` value).
        let overrides = env_overrides(&TokioBenchRunner::in_worktree("some/worktree"));

        for name in LAUNCHER_TOOLCHAIN_VARS {
            assert_eq!(
                overrides.get(&OsString::from(name)),
                Some(&None),
                "{name} must be removed for a historical checkout: {overrides:?}"
            );
        }
    }

    #[test]
    fn in_dir_keeps_the_launcher_toolchain() {
        // The tool's own workspace builds with the toolchain the caller selected
        // (for example via `cargo +nightly run`), so nothing is removed.
        let overrides = env_overrides(&TokioBenchRunner::in_dir("some/workspace"));
        assert!(overrides.is_empty(), "{overrides:?}");

        let overrides = env_overrides(&TokioBenchRunner::default());
        assert!(overrides.is_empty(), "{overrides:?}");
    }

    #[test]
    // The Windows environment map compares keys with `CompareStringOrdinal`, a
    // foreign function Miri cannot call; the same assertion runs under Linux Miri.
    #[cfg_attr(all(miri, windows), ignore)]
    fn injected_environment_wins_over_the_detachment() {
        // The injected engine environment is applied last, so a caller that sets
        // one of these variables explicitly still gets its value through.
        let runner = TokioBenchRunner::in_worktree("some/worktree");
        let argv = vec!["cargo".to_owned(), "bench".to_owned()];
        let env = vec![("CARGO".to_owned(), "/custom/cargo".to_owned())];
        let command = runner.command(&argv, &env);
        let overrides: HashMap<OsString, Option<OsString>> = command
            .as_std()
            .get_envs()
            .map(|(name, value)| (name.to_owned(), value.map(ToOwned::to_owned)))
            .collect();

        assert_eq!(
            overrides.get(&OsString::from("CARGO")),
            Some(&Some(OsString::from("/custom/cargo"))),
            "{overrides:?}"
        );
    }

    #[test]
    fn engine_command_runs_argv_directly() {
        // A forwarded argument with a space stays a single argument, proving no
        // shell re-tokenization happens.
        let argv = vec!["echo".to_owned(), "hi there".to_owned()];
        let command = engine_command(&argv);
        let std = command.as_std();
        let program = std.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = std
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(program, "echo");
        assert_eq!(args, vec!["hi there".to_owned()]);
    }
}
