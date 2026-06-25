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

/// Prints the full report: a summary block followed by the per-mode table.
pub(crate) fn print(
    storage_label: &str,
    scenario: Scenario,
    set_count: usize,
    stats: SeedStats,
    phases: Phases,
    results: &[MeasureResult],
) {
    println!();
    println!("cargo-bench-history stress results");
    println!("==================================");
    println!("storage:          {storage_label}");
    println!("discriminant sets: {set_count}");
    println!("benchmarks / set: {}", scenario.benchmarks);
    println!("main commits:     {}", scenario.commits);
    println!("  with a run:     {}", scenario.commits_with_runs());
    println!("branch commits:   {}", scenario.branch_commits);
    println!("dirty snapshots:  {}", scenario.dirty_runs);
    println!("objects seeded:   {}", stats.objects);
    println!("series defined:   {}", stats.series);
    println!("data seeded:      {}", human_bytes(stats.bytes));
    println!("repo build:       {}", seconds(phases.repo));
    println!("generate + write: {}", seconds(phases.seed));
    println!("upload:           {}", seconds(phases.upload));
    println!();

    println!(
        "{:<9} {:>10} {:>9} {:>8} {:>12} {:>13} {:>8}",
        "mode", "duration", "objects", "series", "regressions", "improvements", "notable"
    );
    println!(
        "{:<9} {:>10} {:>9} {:>8} {:>12} {:>13} {:>8}",
        "----", "--------", "-------", "------", "-----------", "------------", "-------"
    );
    for result in results {
        println!(
            "{:<9} {:>10} {:>9} {:>8} {:>12} {:>13} {:>8}",
            result.mode.keyword(),
            seconds(result.duration),
            result.runs,
            result.series,
            result.regressions,
            result.improvements,
            if result.notable { "yes" } else { "no" },
        );
    }
    println!();
}

/// Formats a duration as seconds with millisecond precision.
fn seconds(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
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
}
