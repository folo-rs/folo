//! Native symlink observations through the public harvesting API.

#![cfg(all(unix, not(miri)))]

use std::fs;
use std::os::unix::fs::symlink;

use cbh_diag::RecordingReporter;
use cbh_engines::{
    BenchOutputSource, FsBenchOutputSource, Harvest, RawCriterionCase, RawOperationFile, RawSummary,
};
use cbh_model::Engine;
use tempfile::tempdir;

::testing::set_allocator!();

#[tokio::test]
async fn recursive_collectors_read_file_links_without_descending_into_directory_links() {
    for (engine, engine_dir, filename) in [
        (Engine::Callgrind, "gungraun", "summary.json"),
        (Engine::Criterion, "criterion", "estimates.json"),
    ] {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let originals = dir.path().join("originals");
        let outside_case = originals.join("new");
        let selected_case = target.join(engine_dir).join("new");
        fs::create_dir_all(&outside_case).unwrap();
        fs::create_dir_all(&selected_case).unwrap();
        fs::write(outside_case.join(filename), "result").unwrap();
        fs::write(outside_case.join("benchmark.json"), "identity").unwrap();
        symlink(outside_case.join(filename), selected_case.join(filename)).unwrap();
        symlink(&originals, target.join(engine_dir).join("linked_group")).unwrap();
        if engine == Engine::Criterion {
            symlink(
                outside_case.join("benchmark.json"),
                selected_case.join("benchmark.json"),
            )
            .unwrap();
        }

        let result = FsBenchOutputSource::new(&target)
            .collect(engine, None, &RecordingReporter::new())
            .await
            .unwrap();
        let expected = match engine {
            Engine::Callgrind => Harvest::Callgrind(vec![RawSummary {
                path: selected_case.join(filename),
                content: "result".into(),
            }]),
            Engine::Criterion => Harvest::Criterion(vec![RawCriterionCase {
                dir: selected_case,
                benchmark: "identity".into(),
                estimates: "result".into(),
            }]),
            _ => unreachable!(),
        };
        assert_eq!(result, expected);
    }
}

#[tokio::test]
async fn flat_collectors_ignore_file_links_and_keep_regular_files() {
    for (engine, engine_dir) in [
        (Engine::AllocTracker, "alloc_tracker"),
        (Engine::AllTheTime, "all_the_time"),
    ] {
        let dir = tempdir().unwrap();
        let target = dir.path().join("target");
        let output = target.join(engine_dir);
        let original = dir.path().join("original.json");
        fs::create_dir_all(&output).unwrap();
        fs::write(&original, "linked").unwrap();
        fs::write(output.join("regular.json"), "regular").unwrap();
        symlink(&original, output.join("linked.json")).unwrap();

        let result = FsBenchOutputSource::new(&target)
            .collect(engine, None, &RecordingReporter::new())
            .await
            .unwrap();
        let files = vec![RawOperationFile {
            path: output.join("regular.json"),
            content: "regular".into(),
        }];
        let expected = match engine {
            Engine::AllocTracker => Harvest::AllocTracker(files),
            Engine::AllTheTime => Harvest::AllTheTime(files),
            _ => unreachable!(),
        };
        assert_eq!(result, expected);
    }
}
