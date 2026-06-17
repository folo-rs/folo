//! The process port: launching engine commands (streaming their output) and
//! capturing the output of helper commands such as `git` and `rustc`.
//!
//! The real adapter uses `tokio::process`; an in-memory fake (in `#[cfg(test)]`)
//! substitutes for it so orchestration is testable without spawning processes.

use std::future::Future;
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};

use crate::bench::merge_wslenv;

/// Launches an engine's benchmark command with injected environment variables.
pub(crate) trait BenchRunner {
    /// Runs `argv` directly (no shell) with `env` applied, letting the child
    /// inherit stdio so benchmark progress streams to the terminal. `argv[0]` is
    /// the program and the remainder are its arguments, each passed verbatim.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be spawned or awaited.
    fn run_engine(
        &self,
        argv: &[String],
        env: &[(String, String)],
    ) -> impl Future<Output = io::Result<EngineStatus>>;
}

/// The exit outcome of an engine command, in a portable shape fakes can build.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct EngineStatus {
    /// Whether the process reported success.
    pub(crate) success: bool,
    /// The process exit code, if one was reported.
    pub(crate) code: Option<i32>,
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
pub(crate) struct CommandOutput {
    /// The process exit status.
    pub(crate) status: ExitStatus,
    /// Captured standard output, lossily decoded as UTF-8.
    pub(crate) stdout: String,
}

/// The real [`BenchRunner`], backed by `tokio::process`.
///
/// By default the engine command runs in the process working directory.
/// `in_dir` runs it in a specific directory (a checked-out worktree) without
/// mutating the process current directory.
#[derive(Clone, Debug, Default)]
pub(crate) struct TokioBenchRunner {
    /// Working directory for the engine command; the process CWD when absent.
    dir: Option<PathBuf>,
}

impl TokioBenchRunner {
    /// Runs the engine command in `dir` instead of the process CWD.
    pub(crate) fn in_dir(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: Some(dir.into()),
        }
    }
}

impl BenchRunner for TokioBenchRunner {
    async fn run_engine(
        &self,
        argv: &[String],
        env: &[(String, String)],
    ) -> io::Result<EngineStatus> {
        let mut command = engine_command(argv);

        if let Some(dir) = self.dir.as_deref() {
            command.current_dir(dir);
        }

        for (name, value) in env {
            command.env(name, value);
        }

        let injected_names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        let existing_wslenv = std::env::var("WSLENV").ok();
        if let Some(wslenv) = merge_wslenv(existing_wslenv.as_deref(), &injected_names) {
            command.env("WSLENV", wslenv);
        }

        Ok(EngineStatus::from_exit(command.status().await?))
    }
}

/// Runs `program` with `args`, capturing its standard output.
///
/// # Errors
///
/// Returns an error if the process cannot be spawned or awaited.
pub(crate) async fn capture(program: &str, args: &[&str]) -> io::Result<CommandOutput> {
    let output = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .await?;

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
        .expect("engine argv is non-empty; build_command_line rejects empty commands");
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    command
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::{TokioBenchRunner, engine_command};

    #[test]
    fn in_dir_sets_the_working_directory() {
        // `in_dir` must record the target directory; the default runner leaves it
        // absent (engine runs in the process CWD), so the two must differ.
        let runner = TokioBenchRunner::in_dir("some/worktree");
        assert_eq!(runner.dir, Some(PathBuf::from("some/worktree")));
        assert_eq!(TokioBenchRunner::default().dir, None);
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
