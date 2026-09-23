//! Filesystem harvesting through the public collector API, with explicit file timestamps.

#![cfg(not(miri))]

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use std::{fs, io};

use cbh_diag::RecordingReporter;
use cbh_engines::{
    BenchOutputSource, FsBenchOutputSource, Harvest, RawCriterionCase, RawOperationFile, RawSummary,
};
use cbh_model::Engine;
use tempfile::tempdir;

/// Output directory written by Gungraun.
const GUNGRAUN_DIR: &str = "gungraun";
/// Output directory written by Criterion.
const CRITERION_DIR: &str = "criterion";
/// Output directory written by the allocation-tracking engine.
const ALLOC_TRACKER_DIR: &str = "alloc_tracker";
/// Output directory written by the processor-time engine.
const ALL_THE_TIME_DIR: &str = "all_the_time";

fn boundary() -> SystemTime {
    // An arbitrary modern whole-second instant supported by both target filesystems.
    // All fixture mtimes are assigned explicitly, so execution never depends on the real clock.
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_000_000_000))
        .unwrap()
}

async fn harvest_with(
    source: &FsBenchOutputSource,
    engine: Engine,
    since: SystemTime,
    reporter: &RecordingReporter,
) -> io::Result<Harvest> {
    source.collect(engine, Some(since), reporter).await
}

async fn harvest(
    source: &FsBenchOutputSource,
    engine: Engine,
    since: SystemTime,
) -> io::Result<Harvest> {
    harvest_with(source, engine, since, &RecordingReporter::new()).await
}

fn write_summary(root: &Path, relative: &str, content: &str) -> PathBuf {
    write_operation_file(root, GUNGRAUN_DIR, relative, content)
}

fn write_criterion_file(root: &Path, relative: &str, content: &str) -> PathBuf {
    write_operation_file(root, CRITERION_DIR, relative, content)
}

fn write_operation_file(root: &Path, engine_dir: &str, name: &str, content: &str) -> PathBuf {
    let path = root.join(engine_dir).join(name);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, content).unwrap();
    set_mtime(&path, boundary());
    path
}

fn set_mtime(path: &Path, when: SystemTime) {
    fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(when)
        .unwrap();
}

fn callgrind_summaries(harvest: Harvest) -> Vec<RawSummary> {
    match harvest {
        Harvest::Callgrind(summaries) => summaries,
        _ => panic!("expected callgrind harvest"),
    }
}

fn criterion_cases(harvest: Harvest) -> Vec<RawCriterionCase> {
    match harvest {
        Harvest::Criterion(cases) => cases,
        _ => panic!("expected criterion harvest"),
    }
}

fn operation_files(harvest: Harvest) -> Vec<RawOperationFile> {
    match harvest {
        Harvest::AllocTracker(files) | Harvest::AllTheTime(files) => files,
        _ => panic!("expected a flat per-operation harvest"),
    }
}

#[tokio::test]
async fn collects_fresh_summaries_recursively() {
    let dir = tempdir().unwrap();
    write_summary(dir.path(), "group_a/summary.json", "a");
    write_summary(dir.path(), "group_b/nested/summary.json", "b");
    write_summary(dir.path(), "group_a/other.json", "ignored");

    let source = FsBenchOutputSource::new(dir.path());
    let since = boundary() - Duration::from_mins(1);
    let harvest = harvest(&source, Engine::Callgrind, since).await.unwrap();

    let summaries = callgrind_summaries(harvest);
    let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
    assert_eq!(contents, vec!["a", "b"]);
}

#[tokio::test]
async fn excludes_summaries_older_than_the_boundary() {
    let dir = tempdir().unwrap();
    let stale = write_summary(dir.path(), "group/summary.json", "stale");

    // Backdate the file well beyond the mtime slack so it is unambiguously stale.
    let long_ago = boundary() - Duration::from_hours(1);
    set_mtime(&stale, long_ago);

    let source = FsBenchOutputSource::new(dir.path());
    let harvest = harvest(&source, Engine::Callgrind, boundary())
        .await
        .unwrap();

    assert!(
        callgrind_summaries(harvest).is_empty(),
        "stale summary should be excluded"
    );
}

#[tokio::test]
async fn disabled_gate_admits_summaries_older_than_any_boundary() {
    let dir = tempdir().unwrap();
    let stale = write_summary(dir.path(), "group/summary.json", "stale");

    // A gated harvest at the fixture boundary drops this file; a disabled gate admits it.
    set_mtime(&stale, boundary() - Duration::from_hours(1));

    let source = FsBenchOutputSource::new(dir.path());
    let harvest = source
        .collect(Engine::Callgrind, None, &RecordingReporter::new())
        .await
        .unwrap();

    let summaries = callgrind_summaries(harvest);
    let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
    assert_eq!(contents, vec!["stale"]);
}

#[tokio::test]
async fn slack_window_includes_just_inside_and_excludes_just_outside() {
    let dir = tempdir().unwrap();
    let inside = write_summary(dir.path(), "fresh/summary.json", "inside");
    let outside = write_summary(dir.path(), "stale/summary.json", "outside");

    // Straddle the freshness slack to distinguish inclusion from exclusion without a clock.
    let since = boundary();
    set_mtime(&inside, since - Duration::from_secs(1));
    set_mtime(&outside, since - Duration::from_secs(3));

    let source = FsBenchOutputSource::new(dir.path());
    let harvest = harvest(&source, Engine::Callgrind, since).await.unwrap();

    let summaries = callgrind_summaries(harvest);
    let contents: Vec<&str> = summaries.iter().map(|s| s.content.as_str()).collect();
    assert_eq!(contents, vec!["inside"]);
}

#[tokio::test]
async fn missing_output_tree_yields_no_summaries() {
    let dir = tempdir().unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let harvest = harvest(&source, Engine::Callgrind, boundary())
        .await
        .unwrap();

    assert!(callgrind_summaries(harvest).is_empty());
}

#[tokio::test]
async fn unreadable_output_tree_reports_error() {
    let dir = tempdir().unwrap();
    // A file in place of the engine directory produces a non-NotFound I/O error.
    fs::write(dir.path().join(GUNGRAUN_DIR), "not a directory").unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let error = harvest(&source, Engine::Callgrind, SystemTime::UNIX_EPOCH)
        .await
        .unwrap_err();

    assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[tokio::test]
async fn criterion_collects_fresh_new_dirs_only() {
    let dir = tempdir().unwrap();
    // Complete base/ siblings must be ignored; only new/ result directories are current output.
    write_criterion_file(dir.path(), "grp/std/now/new/benchmark.json", "bm-std");
    write_criterion_file(dir.path(), "grp/std/now/new/estimates.json", "est-std");
    write_criterion_file(dir.path(), "grp/std/now/base/benchmark.json", "bm-base");
    write_criterion_file(dir.path(), "grp/std/now/base/estimates.json", "est-base");
    write_criterion_file(dir.path(), "grp/fast/now/new/benchmark.json", "bm-fast");
    write_criterion_file(dir.path(), "grp/fast/now/new/estimates.json", "est-fast");

    let source = FsBenchOutputSource::new(dir.path());
    let since = boundary() - Duration::from_mins(1);
    let harvest = harvest(&source, Engine::Criterion, since).await.unwrap();

    let cases = criterion_cases(harvest);
    let pairs: Vec<(&str, &str)> = cases
        .iter()
        .map(|case| (case.benchmark.as_str(), case.estimates.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![("bm-fast", "est-fast"), ("bm-std", "est-std")],
        "only fresh new/ directories should be harvested, sorted by path"
    );
}

#[tokio::test]
async fn criterion_skips_incomplete_and_stale_cases() {
    let dir = tempdir().unwrap();
    // Exercise incomplete, fresh and stale cases in the same output tree.
    write_criterion_file(dir.path(), "grp/incomplete/now/new/benchmark.json", "bm-x");
    write_criterion_file(dir.path(), "grp/fresh/now/new/benchmark.json", "bm-fresh");
    let estimates =
        write_criterion_file(dir.path(), "grp/fresh/now/new/estimates.json", "est-fresh");
    write_criterion_file(dir.path(), "grp/stale/now/new/benchmark.json", "bm-stale");
    let stale_estimates =
        write_criterion_file(dir.path(), "grp/stale/now/new/estimates.json", "est-stale");

    let since = boundary();
    set_mtime(&estimates, since - Duration::from_secs(1));
    set_mtime(&stale_estimates, since - Duration::from_hours(1));

    let source = FsBenchOutputSource::new(dir.path());
    let harvest = harvest(&source, Engine::Criterion, since).await.unwrap();

    let cases = criterion_cases(harvest);
    let benchmarks: Vec<&str> = cases.iter().map(|case| case.benchmark.as_str()).collect();
    assert_eq!(benchmarks, vec!["bm-fresh"]);
}

#[tokio::test]
async fn criterion_missing_tree_yields_no_cases() {
    let dir = tempdir().unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let harvest = harvest(&source, Engine::Criterion, SystemTime::UNIX_EPOCH)
        .await
        .unwrap();

    assert!(criterion_cases(harvest).is_empty());
}

#[tokio::test]
async fn criterion_unreadable_tree_reports_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(CRITERION_DIR), "not a directory").unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let error = harvest(&source, Engine::Criterion, SystemTime::UNIX_EPOCH)
        .await
        .unwrap_err();

    assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[tokio::test]
async fn flat_engine_collects_fresh_top_level_json_only() {
    let dir = tempdir().unwrap();
    // Neither a non-JSON sibling nor a nested file is a flat per-operation result.
    write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "allocate_vec.json", "a");
    write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "grow_map.json", "b");
    write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "notes.txt", "ignore me");
    write_operation_file(dir.path(), ALLOC_TRACKER_DIR, "nested/inner.json", "deep");

    let source = FsBenchOutputSource::new(dir.path());
    let since = boundary() - Duration::from_mins(1);
    let harvest = harvest(&source, Engine::AllocTracker, since).await.unwrap();

    let files = operation_files(harvest);
    let contents: Vec<&str> = files.iter().map(|file| file.content.as_str()).collect();
    assert_eq!(
        contents,
        vec!["a", "b"],
        "only fresh top-level *.json files should be harvested, sorted by path"
    );
}

#[tokio::test]
async fn flat_engine_excludes_files_older_than_the_boundary() {
    let dir = tempdir().unwrap();
    let fresh = write_operation_file(dir.path(), ALL_THE_TIME_DIR, "read_cell.json", "fresh");
    let stale = write_operation_file(dir.path(), ALL_THE_TIME_DIR, "write_cell.json", "stale");

    let since = boundary();
    set_mtime(&fresh, since - Duration::from_secs(1));
    set_mtime(&stale, since - Duration::from_hours(1));

    let source = FsBenchOutputSource::new(dir.path());
    let harvest = harvest(&source, Engine::AllTheTime, since).await.unwrap();

    let files = operation_files(harvest);
    let contents: Vec<&str> = files.iter().map(|file| file.content.as_str()).collect();
    assert_eq!(contents, vec!["fresh"]);
}

#[tokio::test]
async fn flat_engine_missing_tree_yields_no_files() {
    let dir = tempdir().unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let harvest = harvest(&source, Engine::AllocTracker, SystemTime::UNIX_EPOCH)
        .await
        .unwrap();

    assert!(operation_files(harvest).is_empty());
}

#[tokio::test]
async fn flat_engine_unreadable_tree_reports_error() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join(ALLOC_TRACKER_DIR), "not a directory").unwrap();
    let source = FsBenchOutputSource::new(dir.path());

    let error = harvest(&source, Engine::AllocTracker, SystemTime::UNIX_EPOCH)
        .await
        .unwrap_err();

    assert_ne!(error.kind(), io::ErrorKind::NotFound, "{error}");
}

#[tokio::test]
async fn verbose_notes_report_a_missing_engine_directory() {
    let dir = tempdir().unwrap();
    let source = FsBenchOutputSource::new(dir.path());
    let reporter = RecordingReporter::new();

    harvest_with(&source, Engine::Criterion, boundary(), &reporter)
        .await
        .unwrap();

    assert!(
        reporter.contains("does not exist"),
        "an absent engine tree must be reported: {:?}",
        reporter.notes()
    );
    assert!(reporter.contains("harvested 0 fresh cases"));
}

#[tokio::test]
async fn verbose_notes_distinguish_included_and_excluded_cases() {
    let dir = tempdir().unwrap();
    let fresh = write_criterion_file(dir.path(), "grp/fresh/now/new/estimates.json", "est");
    write_criterion_file(dir.path(), "grp/fresh/now/new/benchmark.json", "bm");
    let stale = write_criterion_file(dir.path(), "grp/stale/now/new/estimates.json", "est");
    write_criterion_file(dir.path(), "grp/stale/now/new/benchmark.json", "bm");

    let since = boundary();
    set_mtime(&fresh, since - Duration::from_secs(1));
    set_mtime(&stale, since - Duration::from_hours(1));

    let source = FsBenchOutputSource::new(dir.path());
    let reporter = RecordingReporter::new();
    harvest_with(&source, Engine::Criterion, since, &reporter)
        .await
        .unwrap();

    let notes = reporter.notes();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("including") && n.contains("fresh")),
        "the fresh case must be reported as included: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|n| n.contains("excluding") && n.contains("stale")),
        "the stale case must be reported as excluded: {notes:?}"
    );
}

#[tokio::test]
async fn verbose_notes_report_an_incomplete_criterion_case() {
    let dir = tempdir().unwrap();
    // A new/ directory with only benchmark.json is an incomplete case.
    write_criterion_file(dir.path(), "grp/partial/now/new/benchmark.json", "bm");

    let source = FsBenchOutputSource::new(dir.path());
    let reporter = RecordingReporter::new();
    let harvest = harvest_with(
        &source,
        Engine::Criterion,
        SystemTime::UNIX_EPOCH,
        &reporter,
    )
    .await
    .unwrap();

    assert!(criterion_cases(harvest).is_empty());
    assert!(
        reporter
            .notes()
            .iter()
            .any(|n| n.contains("skipping") && n.contains("partial")),
        "an incomplete case must be reported as skipped: {:?}",
        reporter.notes()
    );
}

#[tokio::test]
async fn verbose_notes_report_included_callgrind_summaries() {
    let dir = tempdir().unwrap();
    write_summary(dir.path(), "group/summary.json", "s");

    let source = FsBenchOutputSource::new(dir.path());
    let since = boundary() - Duration::from_mins(1);
    let reporter = RecordingReporter::new();
    harvest_with(&source, Engine::Callgrind, since, &reporter)
        .await
        .unwrap();

    assert!(
        reporter
            .notes()
            .iter()
            .any(|n| n.contains("including") && n.contains("summary.json")),
        "a fresh summary must be reported as included: {:?}",
        reporter.notes()
    );
    assert!(reporter.contains("harvested 1 fresh summary file"));
}
