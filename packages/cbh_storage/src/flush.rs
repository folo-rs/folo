//! Combining a mutating command's outcome with its post-command cache-invalidation
//! flush, so a failed flush is never silently dropped.

use crate::StorageError;

/// Finishes a mutating command by combining its `command` outcome with the
/// result of the post-command cache-invalidation `flush`, giving a failed flush
/// precedence over the command's own outcome.
///
/// The flush is armed only when a delete or overwrite reached the shared cloud
/// backend during this command, so it is `Ok` in the common case and this simply
/// returns `command` unchanged. When it *does* fail, a mutation already reached
/// the cloud but its invalidation marker did not — a cross-machine
/// cache-correctness hazard that is invisible at the failing command and would
/// leave *other* machines' read-through caches stale. Surfacing it first (rather
/// than after a `command?` that may short-circuit) guarantees the stale-cache
/// failure is never silently dropped, even when the command itself also failed.
///
/// The command's own error type `E` only has to know how to carry a
/// [`StorageError`], so each caller keeps its own aggregate error type (the
/// binary's `RunError`, `cbh_analyze`'s `AnalyzeError`) without this port
/// depending on either.
///
/// # Errors
///
/// Returns the `flush` failure (converted into `E`) when the flush failed, taking
/// precedence over any `command` failure. Otherwise returns the `command` result
/// unchanged.
pub fn finish_with_flush<T, E>(
    command: Result<T, E>,
    flush: Result<(), StorageError>,
) -> Result<T, E>
where
    E: From<StorageError>,
{
    flush?;
    command
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    // A stand-in command error type, so the tests exercise `finish_with_flush`
    // without depending on any consumer's aggregate error type. It carries a flush
    // `StorageError` distinctly from its own command failure, so a test can tell
    // which one came back out.
    #[derive(Debug)]
    enum StandInError {
        Command,
        Storage(StorageError),
    }

    impl From<StorageError> for StandInError {
        fn from(error: StorageError) -> Self {
            Self::Storage(error)
        }
    }

    // A distinct flush failure, so it is unambiguously different from a command
    // failure once both are folded into `StandInError`.
    fn flush_failure() -> StorageError {
        StorageError::NotFound {
            key: "cache-epoch".to_owned(),
        }
    }

    #[test]
    fn returns_the_value_when_both_succeed() {
        let value =
            finish_with_flush::<_, StandInError>(Ok("done"), Ok(())).expect("both succeeded");
        assert_eq!(value, "done");
    }

    #[test]
    fn surfaces_a_flush_failure_after_a_successful_command() {
        // The command reached the cloud, but the invalidation marker write failed —
        // the flush error must not be swallowed just because the command succeeded.
        let error = finish_with_flush::<&str, StandInError>(Ok("done"), Err(flush_failure()))
            .expect_err("the flush failed");
        assert!(
            matches!(error, StandInError::Storage(StorageError::NotFound { .. })),
            "expected the flush error, got {error:?}"
        );
    }

    #[test]
    fn returns_the_command_error_when_the_flush_succeeds() {
        let error = finish_with_flush::<&str, StandInError>(Err(StandInError::Command), Ok(()))
            .expect_err("the command failed");
        assert!(
            matches!(error, StandInError::Command),
            "expected the command error, got {error:?}"
        );
    }

    #[test]
    fn prefers_the_flush_failure_over_a_failed_command() {
        // The regression guard: a delete/overwrite reached the cloud (arming the
        // marker) *and* the command later failed. The flush failure — which leaves
        // other machines' caches stale — must take precedence rather than being
        // silently dropped by the command's own early return.
        let error = finish_with_flush::<&str, StandInError>(
            Err(StandInError::Command),
            Err(flush_failure()),
        )
        .expect_err("both failed");
        assert!(
            matches!(error, StandInError::Storage(StorageError::NotFound { .. })),
            "expected the flush error to win, got {error:?}"
        );
    }
}
