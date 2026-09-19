use std::env::{self, VarError};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use cbh_config::{Config, load_config, resolve_config_path, resolve_project_id};
use cbh_model::sanitize_segment;
use ohno::AppError;
use tempfile::Builder;
use tick::Clock;
use tokio::process::Command as NativeCommand;

use crate::action::errors::{ActionIo, InvalidInput, ProcessFailure};
use crate::action::port::{Host, Output, Process, Publisher};
use crate::cli::Command;
use crate::github::RestGitHub;
use crate::model::Instance;
use crate::operations::{Context, dispatch};
use crate::workflow::files::{append_outputs, canonical_directory, canonical_file};

/// Real filesystem, environment and process primitives for action orchestration.
pub(crate) struct NativeHost;

// Native boundaries have integration coverage; policy runs through Host fakes in unit tests.
#[cfg_attr(test, mutants::skip)]
impl Host for NativeHost {
    fn current_dir(&self) -> Result<PathBuf, AppError> {
        env::current_dir()
            .map_err(|error| ActionIo::caused_by("read working directory", ".", error).into())
    }

    fn environment(&self, name: &str) -> Result<Option<String>, AppError> {
        match env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(VarError::NotPresent) => Ok(None),
            Err(error) => {
                Err(InvalidInput::caused_by(name, "environment value is not Unicode", error).into())
            }
        }
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, AppError> {
        fs::read(path).map_err(|error| ActionIo::caused_by("read file", path, error).into())
    }

    fn directory(&self, path: &Path) -> Result<PathBuf, AppError> {
        let path = fs::canonicalize(path)
            .map_err(|error| ActionIo::caused_by("resolve directory", path, error))?;
        canonical_directory(&path)
    }

    fn output_file(&self, path: &Path) -> Result<PathBuf, AppError> {
        let resolved = resolve_destination(path)?;
        if resolved.exists() {
            canonical_file(&resolved)
        } else {
            let parent = resolved
                .parent()
                .ok_or_else(|| InvalidInput::new("github-output", "requires a parent directory"))?;
            self.directory(parent)?;
            Ok(resolved)
        }
    }

    fn outside_checkout(&self, path: &Path, checkout: &Path) -> Result<(), AppError> {
        let path = resolve_destination(path)?;
        let checkout = self.directory(checkout)?;
        if path.starts_with(&checkout) {
            return Err(InvalidInput::new(
                "temp-dir/cache",
                "analysis artifacts must be outside the checkout",
            )
            .into());
        }
        Ok(())
    }

    fn scratch(&self, root: &Path) -> Result<PathBuf, AppError> {
        let directory = Builder::new()
            .prefix("bench-history-")
            .tempdir_in(root)
            .map_err(|error| ActionIo::caused_by("create report directory", root, error))?;
        // The job uploads these artifacts after the action process exits.
        Ok(directory.keep())
    }

    fn key_files(&self, root: &Path) -> Result<Vec<Vec<u8>>, AppError> {
        let mut directories = vec![self.directory(root)?];
        let mut files = Vec::new();
        while let Some(directory) = directories.pop() {
            let entries = fs::read_dir(&directory).map_err(|error| {
                ActionIo::caused_by("list machine-key directory", &directory, error)
            })?;
            for entry in entries {
                let entry = entry.map_err(|error| {
                    ActionIo::caused_by("read machine-key entry", &directory, error)
                })?;
                let path = entry.path();
                let kind = entry.file_type().map_err(|error| {
                    ActionIo::caused_by("inspect machine-key entry", &path, error)
                })?;
                if kind.is_dir() {
                    directories.push(canonical_directory(&path)?);
                } else if kind.is_file() && entry.file_name() == "machine-key.txt" {
                    files.push(self.read(&canonical_file(&path)?)?);
                } else {
                    return Err(InvalidInput::new(
                        "machine-keys",
                        format!(
                            "expected ordinary machine-key.txt files: {}",
                            path.display()
                        ),
                    )
                    .into());
                }
            }
        }
        Ok(files)
    }

    fn append_outputs(&self, path: &Path, outputs: &str) -> Result<(), AppError> {
        append_outputs(path, outputs)
    }

    fn note(&self, message: &str) {
        eprintln!("[cargo-bench-history-github] {message}");
    }

    async fn instance(&self, cwd: &Path, config: Option<&Path>) -> Result<Instance, AppError> {
        let path = resolve_config_path(cwd, config);
        let config = load_config(&path, config.is_some()).await?;
        project_instance(&config, cwd)
    }

    async fn process(&self, process: &Process) -> Result<String, AppError> {
        let mut command = NativeCommand::new(&process.program);
        command
            .args(&process.args)
            .current_dir(&process.cwd)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        let program = process.program.to_string_lossy();
        if process.output == Output::Capture {
            let output = command
                .stdout(Stdio::piped())
                .output()
                .await
                .map_err(|error| {
                    ProcessFailure::caused_by(&*program, "could not execute", error)
                })?;
            if !output.status.success() {
                return Err(ProcessFailure::new(&*program, output.status.to_string()).into());
            }
            String::from_utf8(output.stdout).map_err(|error| {
                ProcessFailure::caused_by(&*program, "non-UTF-8 machine output", error).into()
            })
        } else {
            let status = command
                .stdout(Stdio::inherit())
                .status()
                .await
                .map_err(|error| {
                    ProcessFailure::caused_by(&*program, "could not execute", error)
                })?;
            if !status.success() {
                return Err(ProcessFailure::new(&*program, status.to_string()).into());
            }
            Ok(String::new())
        }
    }
}

pub(crate) fn project_instance(config: &Config, cwd: &Path) -> Result<Instance, AppError> {
    sanitize_segment(&resolve_project_id(config, cwd)).parse()
}

// Resolve missing output/cache paths through existing ancestors without creating them.
#[cfg_attr(test, mutants::skip)]
fn resolve_destination(path: &Path) -> Result<PathBuf, AppError> {
    let mut ancestor = path;
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for name in suffix.into_iter().rev() {
                    resolved.push(name);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {
                suffix.push(
                    ancestor
                        .file_name()
                        .ok_or_else(|| InvalidInput::new("path", "cannot resolve output path"))?,
                );
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| InvalidInput::new("path", "cannot resolve output parent"))?;
            }
            Err(error) => return Err(ActionIo::caused_by("resolve path", ancestor, error).into()),
        }
    }
}

/// Online construction stays behind the publication port, after the fork and input gates.
pub(crate) struct LivePublisher;

#[cfg_attr(test, mutants::skip)]
impl Publisher for LivePublisher {
    async fn publish(&self, command: Command, context: &Context) -> Result<(), AppError> {
        let github = RestGitHub::from_env()?;
        let clock = Clock::new_tokio();
        dispatch(command, context, &github, &clock).await
    }
}
