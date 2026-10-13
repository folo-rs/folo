//! Native filesystem session-store scenarios, separate from in-process unit tests.

#![cfg(all(windows, feature = "private-test-util", not(miri)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Barrier};
use std::{env, fs, iter, thread};

use dure::test_support::store::{
    Identity as ProcessIdentity, InspectedRecord as RecordFile, Record as SessionRecord,
    Store as FsSessionStore, default_store_root, discard_failed_claim, read_if_present,
    reservation_owner,
};
use dure::{AppCommand, SessionId};
use tempfile::TempDir;
use testing::with_watchdog;

fn store() -> (TempDir, FsSessionStore) {
    let dir = TempDir::new().unwrap();
    let store = FsSessionStore::new(dir.path().to_path_buf());
    (dir, store)
}

fn record(id: SessionId, dir: &Path) -> SessionRecord {
    SessionRecord::new(id, dir)
}

#[test]
fn an_install_failure_removes_its_staging_file() {
    let dir = TempDir::new().unwrap();
    let staging = dir.path().join("failed.claim");
    fs::write(&staging, b"claim").unwrap();
    discard_failed_claim(&staging);

    assert!(!staging.exists());
}

#[test]
fn installing_a_missing_staging_file_reports_the_filesystem_error() {
    let (dir, store) = store();
    store
        .install_claim(&dir.path().join("missing.claim"))
        .unwrap_err();
}

#[test]
fn a_claim_names_its_owner_until_it_is_published() {
    let (dir, store) = store();
    let owner = ProcessIdentity::for_test(7);
    let id = store.allocate_id(&owner).unwrap();

    // A claimed id is not yet a session, so it lists as a reservation only.
    assert_eq!(store.list_reservations().unwrap(), vec![(id, owner)]);
    assert!(store.list().unwrap().is_empty());
    assert!(store.read(id).unwrap().is_none());

    store.publish(&record(id, dir.path())).unwrap();
    assert!(store.list_reservations().unwrap().is_empty());
    assert_eq!(store.list().unwrap().len(), 1);
}

#[test]
fn a_claim_is_deleted_only_by_its_owner() {
    let (_dir, store) = store();
    let owner = ProcessIdentity::for_test(7);
    let id = store.allocate_id(&owner).unwrap();

    store
        .delete_owned_by(id, &ProcessIdentity::for_test(8))
        .unwrap();
    assert_eq!(store.list_reservations().unwrap().len(), 1);

    store.delete_owned_by(id, &owner).unwrap();
    assert!(store.list_reservations().unwrap().is_empty());
}

#[test]
fn a_record_is_deleted_only_by_its_own_supervisor() {
    let (dir, store) = store();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    let mut published = record(id, dir.path());
    published.set_supervisor(ProcessIdentity::new(20, 200));
    store.publish(&published).unwrap();

    // Stands in for a session that took this id after another process read
    // the record it means to delete.
    let stale = ProcessIdentity::new(20, 199);
    store.delete_owned_by(id, &stale).unwrap();
    assert_eq!(store.read(id).unwrap().unwrap(), published);

    store.delete_owned_by(id, &published.supervisor()).unwrap();
    assert!(store.read(id).unwrap().is_none());
}

#[test]
fn a_record_file_that_is_still_empty_reads_as_absent() {
    let (dir, store) = store();
    let id = SessionId::from_u32(3).unwrap();
    // What a claim looks like between its file being created and its owner
    // being written into it.
    fs::write(dir.path().join(format!("{}.json", id.get())), b"").unwrap();

    assert!(store.read(id).unwrap().is_none());
}

#[test]
fn a_record_that_cannot_be_inspected_is_not_deleted() {
    let (dir, store) = store();
    let id = SessionId::from_u32(3).unwrap();
    // Stands in for any record this process cannot read. Failing to inspect
    // one is not evidence that it may be removed.
    fs::create_dir_all(dir.path().join(format!("{}.json", id.get()))).unwrap();

    store
        .delete_owned_by(id, &ProcessIdentity::for_test(7))
        .unwrap_err();
}

#[test]
fn deleting_what_is_not_there_or_not_readable_is_not_an_error() {
    let (dir, store) = store();
    let owner = ProcessIdentity::for_test(7);
    let id = store.allocate_id(&owner).unwrap();
    // A file nothing can parse names nobody, so no owner may delete it.
    fs::write(dir.path().join(format!("{}.json", id.get())), b"not json").unwrap();
    store.delete_owned_by(id, &owner).unwrap();
    assert!(dir.path().join(format!("{}.json", id.get())).exists());

    store
        .delete_owned_by(SessionId::from_u32(99).unwrap(), &owner)
        .unwrap();
}

#[test]
fn allocates_smallest_unused_and_reuses_after_delete() {
    let (_dir, store) = store();
    let first = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    let second = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    assert_eq!(first.get(), 1);
    assert_eq!(second.get(), 2);
    store
        .delete_owned_by(first, &ProcessIdentity::for_test(1))
        .unwrap();
    let reused = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    assert_eq!(reused.get(), 1);
}

#[test]
fn a_reserved_but_unpublished_id_reads_as_absent() {
    let (_dir, store) = store();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    assert!(store.read(id).unwrap().is_none());
}

#[test]
fn a_claimed_id_is_readable_as_a_claim_the_moment_it_exists() {
    // The point of building a claim elsewhere and installing it under the
    // id's name is that the name never exists without its owner behind it.
    // A claim that named nobody could not be proved abandoned, so its id
    // would stay taken for the rest of the logon session.
    let (dir, store) = store();
    let owner = ProcessIdentity::for_test(1);
    let id = store.allocate_id(&owner).unwrap();

    let raw = fs::read(dir.path().join(format!("{}.json", id.get()))).unwrap();
    assert_eq!(reservation_owner(&raw).unwrap(), owner);
    assert_eq!(store.list_reservations().unwrap(), vec![(id, owner)]);
}

#[test]
fn allocating_leaves_no_staging_file_behind() {
    let (dir, store) = store();
    _ = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();

    // A file with no id for a name is one no reader understands, so none
    // may be left in the store.
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| !name.to_string_lossy().ends_with(".json"))
        .collect();
    assert!(leftovers.is_empty(), "left behind {leftovers:?}");
}

#[test]
fn concurrent_allocations_are_unique() {
    with_watchdog(|| {
        const WORKERS: usize = 8;

        let dir = TempDir::new().unwrap();
        let root = dir.path().to_path_buf();
        // Released together, so the allocations overlap. Started one at a
        // time, the first could finish before the last began and the test
        // would prove only that ids differ in sequence.
        let start = Arc::new(Barrier::new(WORKERS));
        let threads: Vec<_> = iter::repeat_with(|| {
            let root = root.clone();
            let start = Arc::clone(&start);
            thread::spawn(move || {
                let store = FsSessionStore::new(root);
                start.wait();
                store.allocate_id(&ProcessIdentity::for_test(1)).unwrap()
            })
        })
        .take(WORKERS)
        .collect();
        let mut ids = HashSet::new();
        for handle in threads {
            assert!(ids.insert(handle.join().unwrap().get()));
        }
        assert_eq!(ids.len(), WORKERS);
    });
}

#[test]
fn publish_read_list_delete() {
    let (dir, store) = store();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    let rec = record(id, dir.path());
    store.publish(&rec).unwrap();
    assert_eq!(store.read(id).unwrap().unwrap(), rec);
    assert_eq!(store.list().unwrap(), vec![rec.clone()]);
    store.delete_owned_by(id, &rec.supervisor()).unwrap();
    assert!(store.read(id).unwrap().is_none());
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn a_record_that_is_already_gone_reads_as_nothing_to_read() {
    // A session can end between a listing and the read that follows it,
    // which is the store working rather than a store this process cannot
    // read.
    let dir = TempDir::new().unwrap();
    assert!(
        read_if_present(&dir.path().join("1.json"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn a_store_that_does_not_exist_yet_holds_no_sessions() {
    // The store root is created when the first session is published, so
    // `list` before that is an empty list rather than a failure.
    let dir = TempDir::new().unwrap();
    let store = FsSessionStore::new(dir.path().join("never-created"));
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn list_skips_files_that_are_not_usable_records() {
    let (dir, store) = store();
    fs::write(dir.path().join("0.json"), b"{\"id\":0}").unwrap();
    fs::write(dir.path().join("not-json.json"), b"nope").unwrap();
    fs::write(dir.path().join("readme.txt"), b"not a record").unwrap();
    // Names a valid id but holds nothing that parses as a session.
    fs::write(dir.path().join("5.json"), b"nope").unwrap();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    let rec = record(id, dir.path());
    store.publish(&rec).unwrap();
    assert_eq!(store.list().unwrap(), vec![rec]);
    assert!(store.list_reservations().unwrap().is_empty());
}

#[test]
fn a_record_the_store_cannot_read_is_reported_rather_than_omitted() {
    let (dir, store) = store();
    // Names a valid session but cannot be read. Omitting it would make an
    // unreadable store look like an empty one.
    fs::create_dir_all(dir.path().join("6.json")).unwrap();

    store.list().unwrap_err();
}

#[test]
fn read_reports_a_corrupt_record() {
    let (dir, store) = store();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    fs::write(dir.path().join(format!("{}.json", id.get())), b"nope").unwrap();
    assert_eq!(
        store.read(id).unwrap_err().kind(),
        std::io::ErrorKind::Other
    );
}

#[test]
fn a_record_path_that_is_not_a_file_is_an_error() {
    let (dir, store) = store();
    let id = SessionId::MIN;
    fs::create_dir_all(dir.path().join(format!("{}.json", id.get()))).unwrap();
    store.read(id).unwrap_err();
    store
        .delete_owned_by(id, &ProcessIdentity::for_test(1))
        .unwrap_err();
}

#[test]
fn a_root_that_is_not_a_directory_is_an_error() {
    let dir = TempDir::new().unwrap();
    let root = dir.path().join("store");
    fs::write(&root, b"not a directory").unwrap();
    let store = FsSessionStore::new(root);

    store.list().unwrap_err();
    store.list_reservations().unwrap_err();
    store
        .allocate_id(&ProcessIdentity::for_test(1))
        .unwrap_err();
    store
        .publish(&record(SessionId::MIN, dir.path()))
        .unwrap_err();
}

#[test]
fn a_staging_path_that_is_not_a_file_fails_the_publish() {
    let (dir, store) = store();
    let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    // Occupies the name `publish` stages the new record under.
    fs::create_dir_all(dir.path().join(format!("{}.json.tmp", id.get()))).unwrap();

    store.publish(&record(id, dir.path())).unwrap_err();
    // The claim survives, so the id is still this owner's to publish under.
    assert_eq!(store.list_reservations().unwrap().len(), 1);
}

#[test]
fn a_record_too_large_to_read_in_one_go_is_still_its_owner_s_to_delete() {
    let (dir, store) = store();
    let owner = ProcessIdentity::for_test(1);
    let id = store.allocate_id(&owner).unwrap();
    // A launch directory and a command line are as long as the user made them, and Windows
    // permits command lines far longer than any convenient buffer size.
    let mut record = record(id, &dir.path().join("d".repeat(9_000)));
    record.set_command(
        AppCommand::from_argv(vec!["app.exe".to_string(), "a".repeat(20_000)])
            .expect("test argv names an executable"),
    );
    store.publish(&record).unwrap();

    // A record read short would parse as nobody's and be declined, stranding the id.
    store.delete_owned_by(id, &record.supervisor()).unwrap();
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn deleting_an_absent_record_succeeds() {
    let (_dir, store) = store();
    store
        .delete_owned_by(SessionId::MIN, &ProcessIdentity::for_test(1))
        .unwrap();
}

#[test]
fn a_record_that_replaced_the_one_inspected_survives_the_delete() {
    let (dir, store) = store();
    let owner = ProcessIdentity::for_test(1);
    let id = store.allocate_id(&owner).unwrap();
    let path = dir.path().join(format!("{}.json", id.get()));

    // Stands in for the record the caller decided to remove, held open so the decision and
    // the removal are separated by exactly the window the ids-are-reused race needs.
    let inspected = RecordFile::open(&path).unwrap();
    // Whoever held the id lets it go and someone else takes it, all under the same name.
    fs::remove_file(&path).unwrap();
    let successor = record(id, dir.path());
    store.publish(&successor).unwrap();

    inspected.delete().unwrap();
    drop(inspected);

    // The delete landed on the file it inspected, not on the name it was reached through.
    assert_eq!(store.list().unwrap().len(), 1);
    // And the successor can still be found and removed by the owner it names.
    store.delete_owned_by(id, &successor.supervisor()).unwrap();
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn canonicalize_resolves_an_existing_path_and_rejects_a_missing_one() {
    let (dir, store) = store();
    let resolved = store.canonicalize(dir.path()).unwrap();
    assert!(resolved.is_absolute());
    assert_eq!(resolved, fs::canonicalize(dir.path()).unwrap());
    store.canonicalize(&dir.path().join("absent")).unwrap_err();
}

#[test]
fn current_directory_comes_from_the_process_not_the_store_root() {
    let (_dir, store) = store();
    assert_eq!(store.current_dir().unwrap(), env::current_dir().unwrap());
}

#[test]
fn read_rejects_filename_id_mismatch() {
    let (dir, store) = store();
    let first = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    let rec = record(first, dir.path());
    store.publish(&rec).unwrap();
    let second = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
    fs::write(
        dir.path().join(format!("{}.json", second.get())),
        rec.stored_bytes(),
    )
    .unwrap();
    assert!(store.read(second).unwrap().is_none());
    assert_eq!(store.list().unwrap(), vec![rec.clone()]);
    store.delete_owned_by(second, &rec.supervisor()).unwrap();
    assert_eq!(store.read(first).unwrap().unwrap(), rec);
}

#[test]
fn missing_root_lists_and_reads_empty() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("no-such-store");
    let store = FsSessionStore::new(missing);
    assert!(store.list().unwrap().is_empty());
    assert!(store.read(SessionId::MIN).unwrap().is_none());
}

#[test]
fn the_default_store_uses_the_per_user_known_folder() {
    let root = default_store_root().unwrap();
    assert!(root.is_absolute());
    assert!(root.ends_with("dure"));
}
