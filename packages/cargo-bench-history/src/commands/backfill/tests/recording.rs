use super::*;

#[test]
fn recorded_commits_explains_each_listing_and_the_union_rule() {
    // The verbose trail must let a reader reconstruct why a commit counted:
    // which prefixes were listed, what each contributed, and which objects were
    // deliberately not counted.
    let storage = MemoryStorage::new();
    store(
        &storage,
        "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/clean.json",
    );
    let reporter = RecordingReporter::new();

    _ = block_on(recorded_commits_in(
        &storage,
        PROJECT,
        &partition(MACHINE),
        &reporter,
    ))
    .unwrap();

    let notes = reporter.notes();
    assert!(
        notes.iter().any(|note| {
            note.contains("v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/")
                && note.contains("1 of them clean")
        }),
        "{notes:?}"
    );
    assert!(
        reporter.contains("unioned rather than intersected"),
        "{notes:?}"
    );
    assert!(reporter.contains("independent data sets"), "{notes:?}");
}

#[test]
fn recorded_commits_counts_any_engine_of_the_partition() {
    let storage = MemoryStorage::new();
    // No rule says which engines a run produces, so a clean result from any one
    // of them means the partition's gap for that commit is filled.
    store(
        &storage,
        "v1/proj/objects/callgrind/x86_64-unknown-linux-gnu/ci/c0/clean.json",
    );
    store(
        &storage,
        "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c1/clean.json",
    );

    assert_eq!(recorded(&storage, MACHINE), ["c0", "c1"]);
}

#[test]
fn recorded_commits_ignores_a_sibling_machine_key() {
    let storage = MemoryStorage::new();
    // `ci-pool` is a different machine and thus a different data set, even
    // though its key starts with this host's `ci`.
    store(
        &storage,
        "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci-pool/c0/clean.json",
    );

    assert!(recorded(&storage, MACHINE).is_empty());
    // The same object is exactly what the sibling machine itself would skip.
    assert_eq!(recorded(&storage, "ci-pool"), ["c0"]);
}

#[test]
fn recorded_commits_ignores_another_target_triple() {
    let storage = MemoryStorage::new();
    // Numbers from another platform say nothing about this one.
    store(
        &storage,
        "v1/proj/objects/criterion/aarch64-apple-darwin/ci/c0/clean.json",
    );

    assert!(recorded(&storage, MACHINE).is_empty());
}

#[test]
fn recorded_commits_ignores_another_project() {
    let storage = MemoryStorage::new();
    store(
        &storage,
        "v1/other/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/clean.json",
    );

    assert!(recorded(&storage, MACHINE).is_empty());
}

#[test]
fn recorded_commits_ignores_objects_that_are_not_clean_results() {
    let storage = MemoryStorage::new();
    // A dirty snapshot is a working-tree measurement and a blessing sidecar is
    // an annotation; neither fills a backfill gap.
    store(
        &storage,
        "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c0/dirty-7.json",
    );
    store(
        &storage,
        "v1/proj/objects/criterion/x86_64-unknown-linux-gnu/ci/c1/bless-7.json",
    );

    assert!(recorded(&storage, MACHINE).is_empty());
}

#[test]
fn recorded_commits_is_empty_when_nothing_is_stored() {
    let storage = MemoryStorage::new();

    assert!(recorded(&storage, MACHINE).is_empty());
}
