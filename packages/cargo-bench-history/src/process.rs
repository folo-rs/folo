//! The process port: launching engine commands (streaming their output) and
//! capturing the output of helper commands such as `git` and `rustc`.
//!
//! The real adapter uses `tokio::process`; an in-memory fake (in `#[cfg(test)]`)
//! substitutes for it so orchestration is testable without spawning processes.

use std::future::Future;
use std::io;
use std::process::{ExitStatus, Stdio};

use crate::bench::merge_wslenv;

/// Launches an engine's benchmark command with injected environment variables.
pub(crate) trait BenchRunner {
    /// Runs `command_line` through the platform shell with `env` applied, letting
    /// the child inherit stdio so benchmark progress streams to the terminal.
    ///
    /// # Errors
    ///
    /// Returns an error if the process cannot be spawned or awaited.
    fn run_engine(
        &self,
        command_line: &str,
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
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TokioBenchRunner;

impl BenchRunner for TokioBenchRunner {
    async fn run_engine(
        &self,
        command_line: &str,
        env: &[(String, String)],
    ) -> io::Result<EngineStatus> {
        let mut command = shell_command(command_line);

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

/// Builds a platform shell command that runs `command_line` verbatim.
fn shell_command(command_line: &str) -> tokio::process::Command {
    if cfg!(windows) {
        let mut command = tokio::process::Command::new("cmd");
        command.arg("/C").arg(command_line);
        command
    } else {
        let mut command = tokio::process::Command::new("sh");
        command.arg("-c").arg(command_line);
        command
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::shell_command;

    #[test]
    fn shell_command_wraps_the_line_for_the_platform() {
        let command = shell_command("echo hi");
        let std = command.as_std();
        let program = std.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = std
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        if cfg!(windows) {
            assert_eq!(program, "cmd");
            assert_eq!(args, vec!["/C".to_owned(), "echo hi".to_owned()]);
        } else {
            assert_eq!(program, "sh");
            assert_eq!(args, vec!["-c".to_owned(), "echo hi".to_owned()]);
        }
    }
}
