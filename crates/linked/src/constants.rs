// A poisoned lock means the process is in an unrecoverable/unsafe state and must exit (we panic).
pub(crate) const ERR_POISONED_LOCK: &str = "encountered poisoned lock";
