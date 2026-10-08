//! Configured exclusions preserve evidence while narrowing the statistical scope.

use cbh_command::AnalyzeOptions;
use cbh_config::{Config, parse_config};
use cbh_detect::Series;
use cbh_diag::RecordingReporter;
use cbh_model::{BenchmarkId, BenchmarkIdPrefix, DiscriminantSet, Engine, MetricKind};
use cbh_storage::MemoryStorage;
use nonempty::nonempty;
use serde_json::json;

use super::harness::*;
use crate::pipeline::exclude_ignored;
use crate::testing::{store_run as store, two_commit_history};

#[test]
fn ignore_filter_retains_unmatched_series_and_counts_metrics_across_partitions() {
    let mut series: Vec<_> = [
        ("family/noisy", "m1", MetricKind::InstructionCount),
        ("family/noisy", "m1", MetricKind::ConditionalBranches),
        ("family/noisy", "m2", MetricKind::InstructionCount),
        ("family_keep", "m1", MetricKind::InstructionCount),
    ]
    .into_iter()
    .map(|(name, machine, kind)| Series {
        set: DiscriminantSet {
            engine: Engine::Callgrind,
            target_triple: "x86_64-unknown-linux-gnu".into(),
            machine_key: machine.into(),
        },
        id: BenchmarkId::new(nonempty![name.to_owned()]),
        kind,
        points: Vec::new(),
        base_window: Vec::new(),
        base_history_count: 0,
        active_start: 0,
        blessing: None,
    })
    .collect();
    let reporter = RecordingReporter::new();
    assert_eq!(exclude_ignored(&mut series, &[], &reporter), 0);
    assert_eq!(series.len(), 4);
    let prefixes = ["family/", "family/noisy", "family/"]
        .map(|prefix| BenchmarkIdPrefix::new(prefix).unwrap());
    assert_eq!(exclude_ignored(&mut series, &prefixes, &reporter), 3);
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].id.qualified(), "family_keep");
    assert!(reporter.contains("matches configured ignore prefix \"family/\""));
    assert!(reporter.contains("excluded 3 series by configuration, leaving 1 series"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Full partitioned analysis exceeds Miri's budget; compact filter tests cover Miri."
)]
fn ignores_count_selected_metric_series_once_and_preserve_ghost_precedence() {
    let storage = MemoryStorage::new();
    store(
        &storage,
        &clean_key("c0"),
        &multi_bench(0, "c0", &[("skip/ghost", 100.0)]),
    );
    let machines: &[&str] = &["m1", "m2"];
    for &machine in machines {
        let mut run = two_metric_set(3, "c3", 100.0, 200.0);
        if machine == "m1" {
            run.results
                .extend(multi_bench(3, "c3", &[("kept", 100.0), ("outside", 100.0)]).results);
        }
        store(
            &storage,
            &clean_key_in("callgrind", "x86_64-unknown-linux-gnu", machine, "c3"),
            &run,
        );
    }
    let git = two_commit_history("c0", "c3");
    let options = AnalyzeOptions {
        machine_key: vec!["all".to_owned()],
        prefixes: ["nm/", "skip/", "kept"]
            .map(|prefix| BenchmarkIdPrefix::new(prefix).unwrap())
            .to_vec(),
        ..options()
    };
    let config =
        parse_config("[ignore]\nbenchmarks = ['nm/', 'nm/nm::observe', 'nm/', 'skip/']").unwrap();
    let (report, reporter) = analyze_configured_json(&git, &storage, &config, &options);
    assert_eq!(report["series"], 1);
    let ignored = machines.len() * 2;
    assert_eq!(report["census"]["total"], ignored + 2);
    assert_eq!(report["census"]["in_scope"], 1);
    assert_eq!(
        report["census"]["reasons"][0],
        json!({"reason": "ghost", "count": 1})
    );
    assert_eq!(
        report["census"]["reasons"][1],
        json!({"reason": "ignored", "count": ignored})
    );
    assert_eq!(report["outcome"], "insufficient_baseline");
    assert!(reporter.contains("matches configured ignore prefix \"nm/\""));
    assert!(reporter.contains(&format!(
        "ignore filter: excluded {ignored} series by configuration"
    )));
}

#[test]
fn ignores_explain_empty_scope_without_ghost_or_baseline_advice() {
    let storage = MemoryStorage::new();
    store(&storage, &clean_key("c3"), &ir_set(3, "c3", 100.0));
    let config = parse_config("[ignore]\nbenchmarks = ['nm/']").unwrap();
    let (report, reporter) = analyze_configured_json(&linear_git(), &storage, &config, &options());
    assert_eq!(report["series"], 0);
    assert_eq!(report["runs"], 1);
    assert_eq!(report["census"]["total"], 1);
    assert_eq!(report["census"]["in_scope"], 0);
    assert_eq!(report["outcome"], "nothing_in_scope");
    assert_eq!(report["ghosts_excluded"], 0);
    assert!(
        report["hint"]
            .as_str()
            .unwrap()
            .contains("[ignore].benchmarks")
    );
    assert!(!reporter.contains("baseline guidance:"));
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Full detector histories; compact selection tests retain Miri coverage."
)]
fn ignores_remove_series_from_both_modes_statistical_families() {
    for branch in [false, true] {
        let storage = MemoryStorage::new();
        let retained = MemoryStorage::new();
        for index in 0..HISTORY_COMMITS {
            let commit = commit_name(index);
            let value = if !branch && index >= REGIME_COMMITS {
                130.0
            } else {
                100.0
            };
            let second = i64::try_from(index).unwrap();
            store(
                &storage,
                &clean_key(&commit),
                &multi_bench(second, &commit, &[("kept", value), ("ignored", value)]),
            );
            store(
                &retained,
                &clean_key(&commit),
                &multi_bench(second, &commit, &[("kept", value)]),
            );
        }
        if branch {
            store(
                &storage,
                &dirty_key("f2", 20),
                &multi_bench(20, "f2", &[("kept", 130.0), ("ignored", 130.0)]),
            );
            store(
                &retained,
                &dirty_key("f2", 20),
                &multi_bench(20, "f2", &[("kept", 130.0)]),
            );
        }
        let git = if branch { branch_git() } else { history_git() };
        let config = parse_config("[ignore]\nbenchmarks = ['ignored']").unwrap();
        let (filtered, _) = analyze_configured_json(&git, &storage, &config, &options());
        let (expected, _) =
            analyze_configured_json(&git, &retained, &Config::default(), &options());
        let (restored, _) = analyze_configured_json(&git, &storage, &Config::default(), &options());
        assert_eq!(filtered["findings"], expected["findings"]);
        assert_eq!(filtered["sets"], expected["sets"]);
        assert_eq!(filtered["regressions"], 1);
        assert_eq!(filtered["census"]["judged"], 1);
        assert_eq!(filtered["census"]["in_scope"], 1);
        assert_eq!(restored["regressions"], 2);
        assert_eq!(restored["census"]["judged"], 2);
    }
}
