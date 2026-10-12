//! `dure list`.

use ohno::AppError;

use crate::OutputFailedError;
use crate::gc::live_sessions;
use crate::list_fmt::format_list;
use crate::output::print_line;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;
use crate::trace::{Trace, trace};

/// Print live sessions.
///
/// `now_unix_ms` arrives from the caller rather than being read here, so the
/// whole rendered table is a function of its inputs
/// (docs/implementation.md, "Session age").
pub(crate) fn execute(
    store: &impl SessionStore,
    processes: &impl Processes,
    now_unix_ms: u64,
    trace: Trace,
) -> Result<(), AppError> {
    let live = live_sessions(store, processes, trace)?;
    trace!(
        trace,
        "ages are measured against unix time {now_unix_ms} ms"
    );
    print_line(format_args!("{}", format_list(&live, now_unix_ms)))
        .map_err(OutputFailedError::caused_by)?;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::AppCommand;
    use crate::pal::processes::{MockProcesses, ProcessLiveness};
    use crate::pal::session_store::{MemorySessionStore, SessionStore};
    use crate::protocol::PROTOCOL_VERSION;
    use crate::session_record::{ProcessIdentity, SessionRecord};

    /// A reading of the clock with no structure of its own; the age column has
    /// its own tests in `list_fmt`.
    const SOME_NOW_MS: u64 = 60_000;

    fn publish_session(store: &MemorySessionStore, dir: &Path) {
        let id = store.allocate_id(&ProcessIdentity::for_test(1)).unwrap();
        store
            .publish(&SessionRecord {
                id,
                supervisor: ProcessIdentity {
                    pid: 10_u32.try_into().unwrap(),
                    creation_time: 100,
                },
                pipe_name: "pipe".to_string(),
                launch_directory: dir.to_path_buf(),
                command: AppCommand::for_test(&["app.exe"]),
                started_at_unix_ms: 1,
                attached: false,
                protocol_version: PROTOCOL_VERSION,
            })
            .unwrap();
    }

    #[test]
    fn empty_store_succeeds() {
        let store = MemorySessionStore::new();
        let processes = MockProcesses::new();
        execute(&store, &processes, SOME_NOW_MS, Trace::default()).unwrap();
    }

    #[test]
    fn live_session_is_kept() {
        let store = MemorySessionStore::new();
        publish_session(&store, Path::new("/work"));
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Live);
        execute(&store, &processes, SOME_NOW_MS, Trace::default()).unwrap();
        assert_eq!(store.list().unwrap().len(), 1);
    }

    #[test]
    fn dead_session_is_reaped() {
        let store = MemorySessionStore::new();
        publish_session(&store, Path::new("/work"));
        let mut processes = MockProcesses::new();
        processes
            .expect_probe()
            .returning(|_| ProcessLiveness::Dead);
        execute(&store, &processes, SOME_NOW_MS, Trace::default()).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
