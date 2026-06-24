//! Drives the `analyze` command once per mode and times it.
//!
//! Each mode is run through the real public [`run_with_overrides`] entry point, so
//! the measured path is exactly production data-loading and detection — only the
//! data underneath is synthetic. The facet filters are forced to `all` because the
//! seeded triples and the `synthetic` machine key never match the host the harness
//! runs on; without that every object would be filtered out.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cargo_bench_history::{AnalyzeOptions, Command, Overrides, RunOutcome, run_with_overrides};
use jiff::Timestamp;
use serde::Deserialize;

use crate::cli::ModeArg;
use crate::error::{Error, fail};
use crate::logging::Logger;
use crate::scenario::{BRANCH_FEATURE, BRANCH_MAIN};

/// A far-past `--since` cutoff guaranteeing the whole history is in window for
/// history-mode analysis (whose default look-back is only six months).
const FULL_HISTORY_SINCE: &str = "2000-01-01";

/// The timed result of analyzing one mode.
#[derive(Clone, Copy, Debug)]
pub(crate) struct MeasureResult {
    /// Which mode was measured.
    pub(crate) mode: ModeArg,
    /// The fastest observed wall-clock time across the repeats.
    pub(crate) duration: Duration,
    /// Stored objects loaded into the analysis.
    pub(crate) runs: usize,
    /// Distinct series compared.
    pub(crate) series: usize,
    /// Flagged regressions.
    pub(crate) regressions: usize,
    /// Flagged improvements.
    pub(crate) improvements: usize,
    /// Whether any finding survived (the downstream signal).
    pub(crate) notable: bool,
}

/// The subset of the analyze JSON report the harness reports on.
#[derive(Debug, Deserialize)]
struct ReportCounts {
    /// Stored objects loaded.
    runs: usize,
    /// Distinct series compared.
    series: usize,
    /// Flagged regressions.
    regressions: usize,
    /// Flagged improvements.
    improvements: usize,
    /// Whether any finding survived.
    notable: bool,
}

/// Runs `analyze` for `mode` `repeat` times, returning the fastest run's timing
/// and the (identical) counts it reported.
///
/// # Errors
///
/// Returns an error if the analysis fails or its report cannot be parsed.
pub(crate) async fn measure(
    workspace: &Path,
    repo: &Path,
    mode: ModeArg,
    anchor: Timestamp,
    repeat: usize,
    logger: Logger,
) -> Result<MeasureResult, Error> {
    let options = build_options(workspace, repo, mode);
    logger.step(&format!("analyzing in {} mode", mode.keyword()));
    logger.detail(&format!(
        "context={}, base={}, facets forced to all (seeded triples and the synthetic machine key \
         never match the host), repeats={repeat}",
        options.context.as_deref().unwrap_or(""),
        options.base.as_deref().unwrap_or(""),
    ));

    let command = Command::Analyze(options);
    let mut fastest: Option<Duration> = None;
    let mut counts: Option<ReportCounts> = None;
    for attempt in 0..repeat.max(1) {
        let started = Instant::now();
        let outcome = run_with_overrides(&command, overrides(workspace, anchor))
            .await
            .map_err(|error| fail(format!("{} analysis failed: {error}", mode.keyword())))?;
        let elapsed = started.elapsed();
        let RunOutcome::Analyzed { report, .. } = outcome else {
            return Err(fail("analyze did not return an Analyzed outcome"));
        };
        let parsed: ReportCounts = serde_json::from_str(&report)
            .map_err(|error| fail(format!("failed to parse the analyze report: {error}")))?;
        logger.detail(&format!(
            "attempt {} took {:.3}s ({} objects, {} series)",
            attempt.saturating_add(1),
            elapsed.as_secs_f64(),
            parsed.runs,
            parsed.series,
        ));
        fastest = Some(fastest.map_or(elapsed, |best| best.min(elapsed)));
        counts = Some(parsed);
    }

    let duration = fastest.ok_or_else(|| fail("no analysis attempts ran"))?;
    let counts = counts.ok_or_else(|| fail("no analysis attempts ran"))?;
    Ok(MeasureResult {
        mode,
        duration,
        runs: counts.runs,
        series: counts.series,
        regressions: counts.regressions,
        improvements: counts.improvements,
        notable: counts.notable,
    })
}

/// Builds the analyze options for `mode`.
fn build_options(workspace: &Path, repo: &Path, mode: ModeArg) -> AnalyzeOptions {
    let (context, base, since) = match mode {
        ModeArg::History => (
            BRANCH_MAIN,
            BRANCH_MAIN,
            Some(FULL_HISTORY_SINCE.to_owned()),
        ),
        ModeArg::Branch => (BRANCH_FEATURE, BRANCH_MAIN, None),
        ModeArg::Tip => (BRANCH_MAIN, BRANCH_MAIN, None),
    };
    AnalyzeOptions {
        config_path: Some(config_path(workspace)),
        repo: Some(repo.to_path_buf()),
        context: Some(context.to_owned()),
        base: Some(base.to_owned()),
        no_dirty: false,
        since,
        until: None,
        engine: vec!["all".to_owned()],
        target_triple: vec!["all".to_owned()],
        machine_key: vec!["all".to_owned()],
        prefixes: Vec::new(),
        format: Some("json".to_owned()),
        mode: Some(mode.keyword().to_owned()),
        include_improvements: true,
        include_inactive: false,
        verbose: false,
    }
}

/// The overrides anchoring the analysis to the seeded workspace and clock.
fn overrides(workspace: &Path, anchor: Timestamp) -> Overrides {
    Overrides {
        workspace_dir: Some(workspace.to_path_buf()),
        target_root: None,
        bench_command: None,
        now: Some(anchor),
    }
}

/// The path to the seeded configuration file under `workspace`.
pub(crate) fn config_path(workspace: &Path) -> PathBuf {
    workspace.join(".cargo").join("bench_history.toml")
}
