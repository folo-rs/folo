use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fmt::Write as _;
use std::path::Path;

use cbh_config::rebase;
use ohno::AppError;
use serde::Deserialize;

use crate::action::args::ActionArgs;
use crate::action::artifact_path::for_output;
use crate::action::environment::Environment;
use crate::action::errors::{InvalidInput, InvalidOutput};
use crate::action::flags::compiler_environment;
use crate::action::inputs::{ActionCommand, Inputs};
use crate::action::native::{LivePublisher, NativeHost};
use crate::action::plan::{Reports, analysis_process, build_process};
use crate::action::port::{Host, Output, Process, Publisher};
use crate::action::publication::publication;
use crate::model::CommitSha;
use crate::result::{AnalysisMode, AnalysisReport, Evidence};
use crate::workflow::projection::report_outputs;
use crate::workflow::receipt::machine_key;

/// Connects the installed root-action command to native effects and lazy publication setup.
// Selecting native adapters is integration-only; run_with retains in-process mutation coverage.
// tests/action.rs checks real dispatch and failure propagation.
// Ref: workspace docs/testing.md, "Mutation testing coverage and skipping mutations".
#[cfg_attr(test, mutants::skip)]
pub(crate) async fn run(args: ActionArgs) -> Result<(), AppError> {
    run_with(args, &NativeHost, &LivePublisher).await
}

/// Executes one validated action invocation against independently supplied effect providers.
///
/// This owns measured-checkout selection, the fork gate and success-only output emission.
/// Fake-driven tests use the same orchestration as the native entry point.
pub(crate) async fn run_with(
    args: ActionArgs,
    host: &impl Host,
    publisher: &impl Publisher,
) -> Result<(), AppError> {
    let invocation_dir = host.current_dir()?;
    let inputs = Inputs::parse(&host.read(&rebase(&invocation_dir, args.inputs_file))?)?;
    let cwd = host.directory(&inputs.get("working-directory").map_or_else(
        || invocation_dir.clone(),
        |path| rebase(&invocation_dir, path.into()),
    ))?;
    let output = host.output_file(&rebase(&cwd, args.github_output))?;
    let environment = Environment::read(host, &invocation_dir)?;
    let instance = host
        .instance(&cwd, inputs.get("config").map(Path::new))
        .await?;
    let mut outputs = format!("instance={}\n", instance.as_str());
    if environment.fork() {
        host.note("Skipping fork-origin pull request: benchmark storage and GitHub publication require a same-repository head.");
        outputs.push_str("skipped=true\nskip-reason=fork-pull-request\n");
        return host.append_outputs(&output, &outputs);
    }
    let tool = args.tool.map_or_else(
        || OsString::from("cargo-bench-history"),
        |path| rebase(&cwd, path).into_os_string(),
    );
    match inputs.command {
        ActionCommand::Collect | ActionCommand::Backfill => {
            let mut process = build_process(&inputs, &cwd, &tool)?;
            process.env = compiler_environment(inputs.get("rustflags"), host)?;
            host.note(&format!(
                "Running {} in {} with arguments {:?}; the explicit scope, feature and write-mode selections preserve this command's defaults, and benchmark output streams to the job log.",
                process.program.to_string_lossy(), process.cwd.display(), process.args
            ));
            host.process(&process).await?;
            if inputs.command == ActionCommand::Collect {
                let key = host
                    .process(&Process {
                        program: tool,
                        args: vec!["machine-key".into()],
                        cwd,
                        output: Output::Capture,
                        env: process.env,
                    })
                    .await?;
                writeln!(outputs, "machine-key={}", machine_key(key.trim())?)
                    .expect("formatting into a String cannot fail");
            }
        }
        ActionCommand::AnalyzeHistory | ActionCommand::AnalyzePr => {
            outputs.push_str(
                &analyze(&inputs, &cwd, &tool, &rebase(&cwd, args.temp_dir), host).await?,
            );
        }
        ActionCommand::Publish(_, _) | ActionCommand::Alert => {
            let (command, context) = publication(&inputs, &cwd, instance, &environment)?;
            publisher.publish(command, &context).await?;
        }
    }
    host.append_outputs(&output, &outputs)
}

/// Prepares one core analysis pass without mutating checkout history or deriving fake coverage.
///
/// The resolved commit, actual keys and externally owned report directory constrain the
/// subprocess plan; completed artifacts are then validated before any output is exposed.
async fn analyze(
    inputs: &Inputs,
    cwd: &Path,
    tool: &OsStr,
    temp: &Path,
    host: &impl Host,
) -> Result<String, AppError> {
    let shallow = git(host, cwd, &["rev-parse", "--is-shallow-repository"]).await?;
    match shallow.trim() {
        "false" => {}
        "true" => {
            return Err(InvalidInput::new(
                "checkout",
                "analysis requires full Git history; check out with fetch-depth: 0",
            )
            .into());
        }
        _ => return Err(InvalidOutput::new("Git did not report shallow-repository status").into()),
    }
    let checkout = git(host, cwd, &["rev-parse", "--show-toplevel"]).await?;
    let checkout = host.directory(Path::new(checkout.trim()))?;
    let temp = host.directory(temp)?;
    host.outside_checkout(&temp, &checkout)?;
    if let Some(cache) = inputs.get("cache") {
        host.outside_checkout(&rebase(cwd, cache.into()), &checkout)?;
    }
    let context = format!("{}^{{commit}}", inputs.get("context").unwrap_or("HEAD"));
    let commit: CommitSha = git(
        host,
        cwd,
        &["rev-parse", "--verify", "--end-of-options", &context],
    )
    .await?
    .trim()
    .parse()?;
    let keys =
        machine_keys(host.key_files(&rebase(cwd, inputs.required("machine-keys")?.into()))?)?;
    let directory = host.scratch(&temp)?;
    host.outside_checkout(&directory, &checkout)?;
    let reports = Reports::new(&directory);
    host.note(&format!(
        "Resolved context {} to {}; selecting measured keys {:?}, expected platforms {} and completed platforms {}. Coverage comes from platform evidence, not the deduplicated key count. Reports persist in {}.",
        inputs.get("context").unwrap_or("HEAD"), commit.as_str(), keys,
        inputs.required("expected-platforms")?, inputs.required("completed-platforms")?, directory.display()
    ));
    host.process(&analysis_process(
        inputs, cwd, tool, &commit, &keys, &reports,
    ))
    .await?;
    analysis_outputs(inputs, &commit, &reports, host)
}

/// Captures dedicated Git machine output without mixing it with long-running benchmark logs.
pub(crate) async fn git(host: &impl Host, cwd: &Path, args: &[&str]) -> Result<String, AppError> {
    host.process(&Process {
        program: "git".into(),
        args: args.iter().map(OsString::from).collect(),
        cwd: cwd.to_owned(),
        output: Output::Capture,
        env: Vec::new(),
    })
    .await
}

/// Builds deterministic analyzer filters from the selected fingerprint-file contents.
///
/// Duplicate hardware keys do not merge or establish the caller's platform coverage.
pub(crate) fn machine_keys(files: Vec<Vec<u8>>) -> Result<Vec<String>, AppError> {
    let keys = files
        .into_iter()
        .map(|bytes| {
            let text = str::from_utf8(&bytes)
                .map_err(|error| InvalidOutput::caused_by("machine key is not UTF-8", error))?;
            machine_key(text.trim())
        })
        .collect::<Result<BTreeSet<_>, AppError>>()?;
    if keys.is_empty() {
        return Err(InvalidInput::new(
            "machine-keys",
            "requires at least one actual machine-key.txt file",
        )
        .into());
    }
    Ok(keys.into_iter().collect())
}

/// Validates the successful pass's artifacts and projects them into job-local workflow outputs.
///
/// The internal outcome file checks consistency; callers receive its validated verdict value,
/// not another path output. Markdown is checked for presence, not parsed for meaning.
fn analysis_outputs(
    inputs: &Inputs,
    commit: &CommitSha,
    reports: &Reports,
    host: &impl Host,
) -> Result<String, AppError> {
    let json = read_text(host, &reports.json)?;
    let evidence = Evidence {
        report: AnalysisReport::parse(&json, commit)?,
        platforms: inputs.platforms()?,
    };
    evidence
        .report
        .require_mode(if inputs.command == ActionCommand::AnalyzeHistory {
            AnalysisMode::History
        } else {
            AnalysisMode::Branch
        })?;
    let counts: ReportCounts = serde_json::from_str(&json).map_err(|error| {
        InvalidOutput::caused_by("report is missing its regression count", error)
    })?;
    if read_text(host, &reports.outcome)?.trim() != evidence.report.outcome.as_str() {
        return Err(InvalidOutput::new("outcome file disagrees with the JSON report").into());
    }
    for path in [&reports.markdown, &reports.summary] {
        if read_text(host, path)?.trim().is_empty() {
            return Err(InvalidOutput::new("rendered report is blank").into());
        }
    }
    let mut outputs = report_outputs(&evidence);
    writeln!(
        outputs,
        "partial-platform-coverage={}\nregressions={}",
        !evidence.platforms.is_complete(),
        counts.regressions
    )
    .expect("formatting into a String cannot fail");
    for (key, path) in [
        ("report-markdown", &reports.markdown),
        ("report-json", &reports.json),
        ("report-summary", &reports.summary),
    ] {
        let value = for_output(path)?;
        writeln!(outputs, "{key}={value}").expect("formatting into a String cannot fail");
    }
    Ok(outputs)
}

/// Reads a rendered artifact through the host while preserving decoding failures as evidence errors.
fn read_text(host: &impl Host, path: &Path) -> Result<String, AppError> {
    String::from_utf8(host.read(path)?)
        .map_err(|error| InvalidOutput::caused_by("report is not UTF-8", error).into())
}

/// A tool-provided tally, not a second interpretation of its findings.
#[derive(Deserialize)]
struct ReportCounts {
    regressions: usize,
}
