//! Native faults and resource cleanup that do not belong in unit tests.

#![cfg(all(windows, feature = "private-test-util", not(miri)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::sync::{Arc, Barrier};
use std::{fs, thread};

use dure::test_support::native::{CancellationProbe, Listener, UncommittedApp, find_executable};
use dure::test_support::store::Store;
use tempfile::TempDir;
use testing::with_watchdog;

#[test]
fn failed_listener_replacement_closes_the_accepted_peer() {
    with_watchdog(|| {
        let listener = Listener::new();
        let peer = listener.connect();
        let error = listener
            .accept_failing_replacement("injected replacement failure")
            .unwrap_err();
        let cause = error.get_ref().unwrap().source().unwrap();
        assert_eq!(cause.to_string(), "injected replacement failure");
        assert_eq!(
            peer.receive().unwrap_err().kind(),
            std::io::ErrorKind::BrokenPipe
        );
    });
}

#[test]
fn rollback_after_app_output_does_not_wait_for_an_absent_pump() {
    with_watchdog(|| {
        let directory = TempDir::new().unwrap();
        let app = UncommittedApp::spawn(
            dure_test_helper::binary_path(),
            &["flood-output".to_string()],
            directory.path(),
        );
        assert!(!app.read_output().unwrap().unwrap().is_empty());
        // Refusing the startup exchange invokes rollback after a real app has
        // started writing, but without any serving output reader.
        drop(app);
        assert!(
            Store::new(directory.path().to_path_buf())
                .list()
                .unwrap()
                .is_empty()
        );
    });
}

#[test]
fn executable_search_uses_only_the_controlled_search_directory() {
    let directory = TempDir::new().unwrap();
    let app = directory.path().join("controlled.exe");
    fs::copy(dure_test_helper::binary_path(), &app).unwrap();
    assert_eq!(find_executable(directory.path(), "controlled"), Some(app));
    assert!(find_executable(directory.path(), "missing").is_none());
}

#[test]
fn cancellation_refuses_later_operations_including_racing_ones() {
    with_watchdog(|| {
        // Modest native contention, not a stress or throughput measurement.
        const WORKERS: usize = 8;
        const ATTEMPTS: usize = 200;
        let pipe = CancellationProbe::default();
        assert!(pipe.issue());
        let start = Arc::new(Barrier::new(WORKERS + 1));
        let mut workers = Vec::new();
        for _ in 0..WORKERS {
            let pipe = pipe.clone();
            let start = Arc::clone(&start);
            workers.push(thread::spawn(move || {
                start.wait();
                let mut refused = false;
                for _ in 0..ATTEMPTS {
                    if pipe.issue() {
                        assert!(!refused, "issuance cannot resume after cancellation");
                    } else {
                        refused = true;
                    }
                }
            }));
        }
        start.wait();
        pipe.cancel();
        for worker in workers {
            worker.join().unwrap();
        }
        assert!(!pipe.issue());
    });
}
