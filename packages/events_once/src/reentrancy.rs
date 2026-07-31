use std::backtrace::Backtrace;
use std::cell::Cell;

/// The closure that an `inspect_awaiters()` method calls once per awaited event.
type AwaiterInspector<'a> = dyn FnMut(&Backtrace) + 'a;

/// Calls the `inspect_awaiters()` method of the object under test, passing it the provided
/// closure.
type InspectAwaiters<'a> = dyn Fn(&mut AwaiterInspector<'_>) + 'a;

/// Asserts that the `inspect_awaiters()` method of an event pool or event lake tolerates a
/// closure that re-enters the same pool or lake.
///
/// The object under test must already contain at least one awaited event, so that the closure
/// is called at least once.
///
/// `inspect` must call the `inspect_awaiters()` method of the object under test with the closure
/// it receives. `rent_and_drop` must rent an event from the same object and immediately drop
/// both of its endpoints, returning the event to the object.
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) fn assert_inspect_awaiters_is_reentrant(
    inspect: &InspectAwaiters<'_>,
    rent_and_drop: &dyn Fn(),
) {
    // The nested inspection is limited to the first call of the outer closure, so that both
    // inspections observe the same number of awaited events.
    const MAX_DEPTH: usize = 1;

    let depth = Cell::new(0_usize);
    let outer_calls = Cell::new(0_usize);
    let nested_calls = Cell::new(0_usize);

    inspect(&mut |_backtrace| {
        outer_calls.set(outer_calls.get().saturating_add(1));

        // Renting an event and returning it exercises the mutating paths of the pool or lake.
        rent_and_drop();

        if depth.get() < MAX_DEPTH {
            depth.set(depth.get().saturating_add(1));

            inspect(&mut |_backtrace| {
                nested_calls.set(nested_calls.get().saturating_add(1));
            });
        }
    });

    assert!(outer_calls.get() > 0);

    // The nested inspection observes the same awaited events as the outer one because the event
    // that `rent_and_drop` rents is never awaited and therefore never inspected.
    assert_eq!(nested_calls.get(), outer_calls.get());
}
