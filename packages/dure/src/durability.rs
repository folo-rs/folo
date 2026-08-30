//! Whether a session can outlive the client that started it.

/// Whether a started session survives the client that launched it.
///
/// Only the supervisor can answer this: Windows reports a process's job
/// membership to that process alone, and breakaway leaves only the immediate
/// job, so the answer is known only once the supervisor is running
/// (implementation.md, "Job breakaway").
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Durability {
    /// No job object ties the session to the client that started it.
    Durable,
    /// A job object will end the session when the client's job closes.
    TiedToLauncher,
}
