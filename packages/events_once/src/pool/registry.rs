use std::any::type_name;
use std::collections::HashSet;
use std::fmt;
use std::ptr::NonNull;

/// The events that a pool has rented out and not yet taken back.
///
/// `plurality::Pool` does not enumerate its live allocations, so a pool keeps track of the events
/// it has handed out. Only debug builds need this, because its sole consumer is the
/// `inspect_awaiters()` diagnostic API, which is itself only available in debug builds.
///
/// A hash set is used because renting and releasing an event are hot paths even in debug builds,
/// so both registration and deregistration must be O(1). An intrusive list would avoid the
/// allocation but would require extra fields on every event, including the events that never
/// enter a pool at all.
pub(crate) struct EventRegistry<E> {
    events: HashSet<NonNull<E>>,
}

impl<E> EventRegistry<E> {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            events: HashSet::new(),
        }
    }

    pub(crate) fn register(&mut self, event: NonNull<E>) {
        let inserted = self.events.insert(event);

        assert!(inserted, "an event was registered twice");
    }

    pub(crate) fn unregister(&mut self, event: NonNull<E>) {
        let removed = self.events.remove(&event);

        assert!(removed, "an event was released twice");
    }

    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.events.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = NonNull<E>> {
        self.events.iter().copied()
    }
}

// SAFETY: The registry stores plain addresses and nothing else. Moving it to another thread moves
// no event, and reaching an event through one of these addresses requires unsafe code that must
// independently establish that the event is still alive.
unsafe impl<E> Send for EventRegistry<E> {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<E> fmt::Debug for EventRegistry<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("events", &self.events)
            .finish()
    }
}
