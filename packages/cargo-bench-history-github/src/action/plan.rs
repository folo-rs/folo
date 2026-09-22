use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use cbh_config::rebase;
use ohno::AppError;

use crate::action::inputs::{ActionCommand, Inputs};
use crate::action::port::{Output, Process};
use crate::model::CommitSha;

/// Artifact paths are owned by one invocation and survive it for the job's upload step.
#[derive(Debug)]
pub(crate) struct Reports {
    pub(crate) markdown: PathBuf,
    pub(crate) json: PathBuf,
    pub(crate) summary: PathBuf,
    pub(crate) outcome: PathBuf,
}

impl Reports {
    /// Assigns the core pass's report files within the invocation-owned scratch directory.
    pub(crate) fn new(directory: &Path) -> Self {
        Self {
            markdown: directory.join("report.md"),
            json: directory.join("report.json"),
            summary: directory.join("summary.md"),
            outcome: directory.join("outcome.txt"),
        }
    }
}

/// Plans collect/backfill arguments while preserving scope, feature and write-mode choices.
///
/// The host executes this argument vector in the measured checkout with inherited streams;
/// installation-source selection does not choose that checkout.
pub(crate) fn build_process(
    inputs: &Inputs,
    cwd: &Path,
    tool: &OsStr,
) -> Result<Process, AppError> {
    let backfill = inputs.command == ActionCommand::Backfill;
    let mut args = vec![OsString::from(if backfill {
        "backfill"
    } else {
        "collect"
    })];
    if backfill {
        args.extend([
            inputs.required("from")?.into(),
            inputs.required("to")?.into(),
        ]);
    }
    common_arguments(inputs, cwd, &mut args);
    let packages = inputs.list("packages")?;
    if packages.is_empty() {
        args.push("--workspace".into());
    }
    for (input, flag) in [
        ("packages", "--package"),
        ("exclude", "--exclude"),
        ("bench", "--bench"),
        ("features", "--features"),
    ] {
        for item in inputs.list(input)? {
            option(&mut args, flag, item);
        }
    }
    option(&mut args, "--best-of", inputs.get("best-of").unwrap_or("1"));
    for (input, default) in [
        ("all-features", true),
        ("no-default-features", false),
        ("ignore-errors", false),
    ] {
        if inputs.boolean(input, default)? {
            args.push(format!("--{input}").into());
        }
    }
    match inputs
        .get("on-existing")
        .unwrap_or(if backfill { "skip" } else { "error" })
    {
        "overwrite" => args.push("--overwrite".into()),
        "skip" if !backfill => args.push("--skip-existing".into()),
        _ => {}
    }
    Ok(Process {
        program: tool.to_owned(),
        args,
        cwd: cwd.to_owned(),
        output: Output::Inherit,
        env: Vec::new(),
    })
}

/// Plans the core comparison question and artifact destinations from already resolved evidence.
///
/// History uses one commit as context and base; PR analysis leaves an omitted base to the
/// core. Keys are the validated collection filters, not an inference from the analyzer host.
pub(crate) fn analysis_process(
    inputs: &Inputs,
    cwd: &Path,
    tool: &OsStr,
    commit: &CommitSha,
    keys: &[String],
    reports: &Reports,
) -> Process {
    let mut args = vec!["analyze".into()];
    common_arguments(inputs, cwd, &mut args);
    option(&mut args, "--context", commit.as_str());
    if inputs.command == ActionCommand::AnalyzeHistory {
        option(&mut args, "--base", commit.as_str());
    } else if let Some(base) = inputs.get("base") {
        option(&mut args, "--base", base);
    }
    args.extend(["--no-dirty".into(), "--no-text".into()]);
    option(&mut args, "--engine", "all");
    option(&mut args, "--target-triple", "all");
    for key in keys {
        option(&mut args, "--machine-key", key);
    }
    for (flag, path) in [
        ("--markdown", &reports.markdown),
        ("--json", &reports.json),
        ("--markdown-summary", &reports.summary),
        ("--outcome", &reports.outcome),
    ] {
        option(&mut args, flag, path);
    }
    if let Some(since) = inputs.get("since") {
        option(&mut args, "--since", since);
    }
    if let Some(cache) = inputs.get("cache") {
        option(&mut args, "--cache", rebase(cwd, cache.into()));
    }
    Process {
        program: tool.to_owned(),
        args,
        cwd: cwd.to_owned(),
        output: Output::Inherit,
        env: Vec::new(),
    }
}

/// Applies shared core diagnostics and checkout-relative configuration/storage paths.
fn common_arguments(inputs: &Inputs, cwd: &Path, args: &mut Vec<OsString>) {
    args.push("--verbose".into());
    for (input, flag) in [("config", "--config"), ("local-path", "--local")] {
        if let Some(path) = inputs.get(input) {
            option(args, flag, rebase(cwd, path.into()));
        }
    }
}

/// Encodes one value as a single process argument rather than a shell fragment.
fn option(args: &mut Vec<OsString>, flag: &str, value: impl AsRef<OsStr>) {
    // Joined arguments preserve leading dashes as values without invoking any shell.
    let mut argument = OsString::from(flag);
    argument.push("=");
    argument.push(value);
    args.push(argument);
}
