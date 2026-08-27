//! `dure list`.

use ohno::AppError;

use crate::gc::live_sessions;
use crate::list_fmt::format_list;
use crate::pal::processes::Processes;
use crate::pal::session_store::SessionStore;

/// Print live sessions.
pub(crate) fn execute(
    store: &impl SessionStore,
    processes: &impl Processes,
) -> Result<(), AppError> {
    let live = live_sessions(store, processes)?;
    println!("{}", format_list(&live));
    Ok(())
}
