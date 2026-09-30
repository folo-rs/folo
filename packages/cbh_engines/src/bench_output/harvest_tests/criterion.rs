use std::collections::VecDeque;

use cbh_model::Engine;

use super::harness::{Directory, Entry, FakeFiles, boundary, collect, path};
use crate::bench_output::{Harvest, RawCriterionCase};
use crate::output_files::EntryType;

#[test]
fn criterion_pairs_only_complete_new_directories_and_sorts_cases() {
    let mut files = FakeFiles::default();
    let a_benchmark = files.file("criterion/a/new/benchmark.json", "a-id", boundary());
    let a_estimates = files.file("criterion/a/new/estimates.json", "a-est", boundary());
    let mut z_benchmark = files.file("criterion/z/new/benchmark.json", "z-id", boundary());
    z_benchmark.file_type = Ok(EntryType::Other);
    let z_estimates = files.file("criterion/z/new/estimates.json", "z-est", boundary());
    files.directory(
        "criterion",
        [
            Entry::directory("criterion/a"),
            Entry::directory("criterion/z"),
            Entry::other("criterion/linked_group"),
        ],
    );
    files.directory(
        "criterion/a",
        [
            Entry::directory("criterion/a/new"),
            Entry::directory("criterion/a/base"),
        ],
    );
    files.directory("criterion/z", [Entry::directory("criterion/z/new")]);
    files.directory("criterion/a/new", [a_estimates, a_benchmark]);
    files.directory(
        "criterion/z/new",
        [
            z_benchmark,
            Entry::ignored("criterion/z/new/notes.txt"),
            z_estimates,
        ],
    );
    // Complete base output is structural, not a current case; contents must not be read.
    files.directory(
        "criterion/a/base",
        [
            Entry::file("criterion/a/base/benchmark.json", boundary()),
            Entry::file("criterion/a/base/estimates.json", boundary()),
        ],
    );

    assert_eq!(
        collect(&files, Engine::Criterion, None).unwrap(),
        Harvest::Criterion(vec![
            RawCriterionCase {
                dir: path("criterion/a/new"),
                benchmark: "a-id".into(),
                estimates: "a-est".into(),
            },
            RawCriterionCase {
                dir: path("criterion/z/new"),
                benchmark: "z-id".into(),
                estimates: "z-est".into(),
            },
        ])
    );
    files.assert_consumed();
}

#[test]
fn criterion_requires_both_files_and_does_not_treat_directories_as_files() {
    for entries in [
        vec![],
        vec![Entry::ignored("criterion/new/benchmark.json")],
        vec![Entry::file("criterion/new/estimates.json", boundary())],
        vec![Entry::ignored("criterion/new/other.json")],
        vec![
            Entry::directory("criterion/new/benchmark.json"),
            Entry::file("criterion/new/estimates.json", boundary()),
        ],
    ] {
        let mut files = FakeFiles::default();
        files.directory("criterion", [Entry::directory("criterion/new")]);
        for entry in &entries {
            if entry.file_type.unwrap() == EntryType::Directory {
                files
                    .directories
                    .get_mut()
                    .insert(entry.path.clone(), Ok(Directory(VecDeque::new())));
            }
        }
        files.directory("criterion/new", entries);
        assert_eq!(
            collect(&files, Engine::Criterion, Some(boundary())).unwrap(),
            Harvest::Criterion(vec![])
        );
        files.assert_consumed();
    }
}
