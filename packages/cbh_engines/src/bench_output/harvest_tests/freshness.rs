use std::time::{Duration, SystemTime};

use cbh_model::Engine;

use super::harness::{boundary, candidate, collect, engines, harvest_len};
use crate::bench_output::MTIME_SLACK;

#[test]
fn recursive_freshness_includes_the_cutoff_and_newer_output_but_not_older_output() {
    assert_freshness([Engine::Callgrind, Engine::Criterion]);
}

#[test]
fn flat_freshness_includes_the_cutoff_and_newer_output_but_not_older_output() {
    assert_freshness([Engine::AllocTracker, Engine::AllTheTime]);
}

#[test]
fn disabled_freshness_admits_old_output_for_every_engine() {
    for engine in engines() {
        let files = candidate(engine, SystemTime::UNIX_EPOCH, true);
        assert_eq!(harvest_len(collect(&files, engine, None).unwrap()), 1);
        files.assert_consumed();
    }
}

fn assert_freshness(engines: [Engine; 2]) {
    // Separate collector families keep the per-test Miri workload bounded.
    for engine in engines {
        let cutoff = boundary().checked_sub(MTIME_SLACK).unwrap();
        for (modified, included) in [
            (cutoff.checked_sub(Duration::from_secs(1)).unwrap(), false),
            (cutoff, true),
            (cutoff.checked_add(Duration::from_secs(1)).unwrap(), true),
        ] {
            let files = candidate(engine, modified, included);
            assert_eq!(
                harvest_len(collect(&files, engine, Some(boundary())).unwrap()),
                usize::from(included)
            );
            files.assert_consumed();
        }
    }
}
