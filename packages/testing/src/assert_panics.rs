use std::any::Any;
use std::panic::{self, AssertUnwindSafe};

/// Asserts that the closure panics.
///
/// Captures the panic so the surrounding test can keep running and make
/// further assertions about state after the panic.
///
/// Prefer `#[should_panic]` when the panic is the only thing the test
/// verifies. This helper is for the case where the test must also assert on
/// post-panic state.
///
/// Use [`assert_panics_with`] when the test needs to inspect the panic
/// message (e.g. to check a canary substring).
///
/// # Panics
///
/// Panics if the closure does not panic.
///
/// # Example
///
/// ```rust
/// use std::cell::Cell;
///
/// use testing::assert_panics;
///
/// let counter = Cell::new(0);
/// assert_panics(|| {
///     counter.set(counter.get() + 1);
///     panic!("something went wrong");
/// });
/// assert_eq!(counter.get(), 1);
/// ```
#[track_caller]
pub fn assert_panics<F, R>(f: F)
where
    F: FnOnce() -> R,
{
    let Err(_payload) = panic::catch_unwind(AssertUnwindSafe(f)) else {
        panic!("closure did not panic as expected");
    };
}

/// Asserts that the closure panics and invokes the inspector with the panic
/// message.
///
/// Captures the panic so the surrounding test can keep running and make
/// further assertions about state after the panic. The inspector receives
/// the panic message and can perform additional assertions on it — typically
/// a canary substring check for a stable keyword inherent to the scenario
/// (see the "Canary substrings" exception in `AGENTS.md`). Avoid asserting
/// on the full message.
///
/// If the panic payload is neither `&'static str` nor `String`, the
/// inspector receives an empty string. Standard `panic!()` invocations
/// always produce one of these payload types.
///
/// # Panics
///
/// Panics if the closure does not panic.
///
/// # Example
///
/// ```rust
/// use testing::assert_panics_with;
///
/// assert_panics_with(
///     || {
///         let x: u32 = u32::MAX;
///         _ = x.checked_add(1).expect("overflow detected");
///     },
///     |message| assert!(message.contains("overflow")),
/// );
/// ```
#[track_caller]
pub fn assert_panics_with<F, R, I>(f: F, inspector: I)
where
    F: FnOnce() -> R,
    I: FnOnce(&str),
{
    let Err(payload) = panic::catch_unwind(AssertUnwindSafe(f)) else {
        panic!("closure did not panic as expected");
    };

    let message = extract_panic_message(&*payload);
    inspector(&message);
}

fn extract_panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        String::new()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::panic;

    use super::*;

    #[test]
    fn assert_panics_returns_when_closure_panics() {
        assert_panics(|| panic!("boom"));
    }

    #[test]
    #[should_panic]
    fn assert_panics_panics_when_closure_does_not_panic() {
        assert_panics(|| {
            // Intentionally does not panic.
        });
    }

    #[test]
    fn assert_panics_allows_post_panic_state_assertions() {
        let value = Cell::new(0);
        assert_panics(|| {
            value.set(7);
            panic!("after mutation");
        });
        assert_eq!(value.get(), 7);
    }

    #[test]
    fn assert_panics_with_passes_str_message_to_inspector() {
        let captured = Cell::new(None);
        assert_panics_with(
            || panic!("static string panic"),
            |message| captured.set(Some(message.to_owned())),
        );
        assert_eq!(
            captured.into_inner().as_deref(),
            Some("static string panic")
        );
    }

    #[test]
    fn assert_panics_with_passes_string_message_to_inspector() {
        let captured = Cell::new(None);
        assert_panics_with(
            || {
                let dynamic = format!("dynamic {} panic", 42);
                panic!("{}", dynamic);
            },
            |message| captured.set(Some(message.to_owned())),
        );
        assert_eq!(captured.into_inner().as_deref(), Some("dynamic 42 panic"));
    }

    #[test]
    fn assert_panics_with_passes_empty_string_for_non_string_payload() {
        let captured = Cell::new(None);
        assert_panics_with(
            || panic::panic_any(42_u32),
            |message| captured.set(Some(message.to_owned())),
        );
        assert_eq!(captured.into_inner().as_deref(), Some(""));
    }

    #[test]
    #[should_panic]
    fn assert_panics_with_panics_when_closure_does_not_panic() {
        assert_panics_with(
            || {
                // Intentionally does not panic.
            },
            |_| {},
        );
    }

    #[test]
    #[should_panic]
    fn assert_panics_with_propagates_inspector_panic() {
        assert_panics_with(
            || panic!("the closure panic"),
            |message| assert!(message.contains("nonexistent canary")),
        );
    }
}
