//! `dure list`.

use ohno::AppError;

use crate::gc::live_sessions;
use crate::list_fmt::format_list;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;
use crate::trace::{Trace, trace};
use crate::wall_clock::unix_now_ms;

/// Print live sessions.
pub(crate) fn execute(
    store: &impl SessionStore,
    processes: &impl Processes,
    trace: Trace,
) -> Result<(), AppError> {
    let live = live_sessions(store, processes, trace)?;
    let now = unix_now_ms();
    trace!(trace, "ages are measured against unix time {now} ms");
    println!("{}", format_list(&live, now));
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::pal::processes::{MockProcesses, ProcessLiveness};
    use crate::pal::session_store::{FsSessionStore, SessionStore};
    use crate::session_record::{ProcessIdentity, SessionRecord};

    fn publish_session(store: &FsSessionStore, dir: &Path) {
        let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        store
            .publish(&SessionRecord {
                id: id.get(),
                supervisor_pid: 10,
                supervisor_creation_time: 100,
                pipe_name: "pipe".to_string(),
                launch_directory: dir.to_path_buf(),
                command: vec!["app.exe".to_string()],
                started_at_unix_ms: 1,
                attached: false,
            })
            .unwrap();
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn empty_store_succeeds() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        let processes = MockProcesses::new();
        execute(&store, &processes, Trace::default()).unwrap();
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn live_session_is_kept() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        publish_session(&store, dir.path());
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        execute(&store, &processes, Trace::default()).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    // Talks to the real operating system: the session store is a real directory.
    #[cfg_attr(miri, ignore)]
    fn dead_session_is_reaped() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = FsSessionStore::new(dir.path().to_path_buf());
        publish_session(&store, dir.path());
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Dead);
        execute(&store, &processes, Trace::default()).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
