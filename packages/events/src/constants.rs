// A poisoned lock means the process is in an unrecoverable/unsafe state and must exit (we panic).
pub(crate) const ERR_POISONED_LOCK: &str = "encountered poisoned lock - continued execution \
    is not safe because we can no longer ensure that we uphold security and privacy guarantees";
