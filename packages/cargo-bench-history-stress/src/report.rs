//! Renders the run summary and the per-mode timing table to stdout.
//!
//! Stdout carries only this report, so it can be redirected to a file cleanly
//! while progress logging stays on stderr.

// Cosmetic formatting over tiny, bounded inputs (unit indices, byte scaling): the
// numeric operations here cannot misbehave and exact rounding is irrelevant.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::indexing_slicing,
    reason = "cosmetic report formatting over tiny fixed inputs"
)]

use std::time::Duration;

use crate::measure::MeasureResult;
use crate::scenario::Scenario;
use crate::seed::SeedStats;

/// Wall-clock cost of each seeding phase, for the summary.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Phases {
    /// Time spent building the git repository.
    pub(crate) repo: Duration,
    /// Time spent generating and writing the blob tree.
    pub(crate) seed: Duration,
    /// Time spent uploading (zero for local storage).
    pub(crate) upload: Duration,
}

/// Renders the full report: a summary block followed by the per-mode table.
pub(crate) fn render(
    storage_label: &str,
    scenario: Scenario,
    set_count: usize,
    stats: SeedStats,
    phases: Phases,
    results: &[MeasureResult],
) -> String {
    let mut lines = vec![
        String::new(),
        "cargo-bench-history stress results".to_owned(),
        "==================================".to_owned(),
        format!("storage:          {storage_label}"),
        format!("discriminant sets: {set_count}"),
        format!("benchmarks / set: {}", scenario.benchmarks),
        format!("main commits:     {}", scenario.commits),
        format!("  with a run:     {}", scenario.commits_with_runs()),
        format!("branch commits:   {}", scenario.branch_commits),
        format!("dirty snapshots:  {}", scenario.dirty_runs),
        format!("objects seeded:   {}", stats.objects),
        format!("series defined:   {}", stats.series),
        format!("data seeded:      {}", human_bytes(stats.bytes)),
        format!("repo build:       {}", seconds(phases.repo)),
        format!("generate + write: {}", seconds(phases.seed)),
        format!("upload:           {}", seconds(phases.upload)),
        String::new(),
        format!(
            "{:<9} {:>10} {:>10} {:>6} {:>9} {:>8} {:>12} {:>13} {:>8}",
            "mode",
            "duration",
            "cpu",
            "cpu%",
            "objects",
            "series",
            "regressions",
            "improvements",
            "notable"
        ),
        format!(
            "{:<9} {:>10} {:>10} {:>6} {:>9} {:>8} {:>12} {:>13} {:>8}",
            "----",
            "--------",
            "----------",
            "----",
            "-------",
            "------",
            "-----------",
            "------------",
            "-------"
        ),
    ];
    for result in results {
        lines.push(format!(
            "{:<9} {:>10} {:>10} {:>6} {:>9} {:>8} {:>12} {:>13} {:>8}",
            result.mode.keyword(),
            seconds(result.duration),
            seconds(result.processor_time),
            percent(result.cpu_efficiency),
            result.runs,
            result.series,
            result.regressions,
            count_or_absent(result.improvements),
            if result.notable { "yes" } else { "no" },
        ));
    }
    lines.push(String::new());
    lines.join("\n")
}

/// Formats a tally the analysis reports, or `n/a` for one it does not look for.
fn count_or_absent(count: Option<usize>) -> String {
    count.map_or_else(|| "n/a".to_owned(), |count| count.to_string())
}

/// Formats a duration as seconds with millisecond precision.
fn seconds(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

/// Formats a `0.0..=1.0` ratio as a whole-number percentage.
fn percent(ratio: f64) -> String {
    format!("{:.0}%", ratio * 100.0)
}

/// Formats a byte count in binary units (KiB, MiB, ...).
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::ModeArg;

    #[test]
    fn report_contains_the_scenario_summary_and_mode_results() {
        let output = render(
            "local",
            Scenario {
                benchmarks: 2,
                commits: 4,
                branch_commits: 1,
                dirty_runs: 0,
                seed: 7,
            },
            3,
            SeedStats {
                objects: 9,
                bytes: 1024,
                series: 6,
            },
            Phases {
                repo: Duration::from_millis(250),
                seed: Duration::from_millis(500),
                upload: Duration::ZERO,
            },
            &[MeasureResult {
                mode: ModeArg::Branch,
                duration: Duration::from_millis(1500),
                processor_time: Duration::from_millis(750),
                cpu_efficiency: 0.5,
                runs: 7,
                series: 6,
                regressions: 2,
                improvements: Some(0),
                notable: true,
            }],
        );

        assert!(output.contains("storage:          local"));
        assert!(output.contains("discriminant sets: 3"));
        assert!(output.contains("data seeded:      1.00 KiB"));
        let row = output
            .lines()
            .map(|line| line.split_whitespace().collect::<Vec<_>>())
            .find(|columns| columns.len() == 9 && columns.first() == Some(&"branch"))
            .expect("the branch result is rendered");
        assert_eq!(
            row,
            [
                "branch", "1.500s", "0.750s", "50%", "7", "6", "2", "0", "yes"
            ]
        );
    }

    #[test]
    fn human_bytes_uses_binary_units() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1.00 KiB");
        assert_eq!(human_bytes(1536), "1.50 KiB");
        assert_eq!(human_bytes(1024 * 1024), "1.00 MiB");
        assert_eq!(human_bytes(1024 * 1024 * 1024), "1.00 GiB");
        assert_eq!(human_bytes(1_u64 << 40), "1.00 TiB");
        // TiB is the largest unit: anything bigger keeps scaling within TiB rather
        // than indexing past the unit table (which the loop's upper bound prevents).
        assert_eq!(human_bytes(1_u64 << 50), "1024.00 TiB");
    }

    #[test]
    fn seconds_renders_millisecond_precision() {
        assert_eq!(seconds(Duration::from_millis(1500)), "1.500s");
        assert_eq!(seconds(Duration::from_secs(0)), "0.000s");
    }

    #[test]
    fn percent_renders_a_whole_number_percentage() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.25), "25%");
        assert_eq!(percent(1.0), "100%");
    }

    #[test]
    fn optional_counts_render_a_value_or_absence() {
        assert_eq!(count_or_absent(None), "n/a");
        assert_eq!(count_or_absent(Some(0)), "0");
        assert_eq!(count_or_absent(Some(7)), "7");
    }
}
