use std::any::type_name;
use std::backtrace::Backtrace;
use std::cell::{RefCell, UnsafeCell};
use std::collections::HashSet;
use std::fmt;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

use crate::{BacktraceType, Event, LocalEvent, NEVER_POISONED};

type SyncBacktraceCell = Mutex<Option<BacktraceType>>;
type LocalBacktraceCell = RefCell<Option<BacktraceType>>;

/// Tracks diagnostics for thread-safe events that have not yet been released.
///
/// Plurality pools do not enumerate their live allocations, so pools and lakes keep this registry
/// alongside their storage. It stores pointers to the type-independent backtrace cells instead of
/// event pointers, which allows one registry to cover every payload type in a lake without its own
/// type router.
///
/// A hash set is used because renting and releasing an event are hot paths even in debug builds,
/// so both registration and deregistration must be O(1). An intrusive list would avoid the
/// allocation but would require extra fields on every event, including the events that never
/// enter a pool at all.
pub(crate) struct EventRegistry {
    backtraces: Mutex<HashSet<NonNull<SyncBacktraceCell>>>,
}

impl EventRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            backtraces: Mutex::new(HashSet::new()),
        }
    }

    /// Registers a live event.
    ///
    /// # Safety
    ///
    /// `event` must point to an initialized event at a stable address. The event must remain alive
    /// at that address, and only shared references to it may be created, until the matching
    /// [`unregister()`][Self::unregister] call returns.
    pub(crate) unsafe fn register<T: Send + 'static>(&self, event: NonNull<UnsafeCell<Event<T>>>) {
        // SAFETY: The caller guarantees that `event` is initialized at a stable address, remains
        // live there until unregistration, and is reached only through shared references throughout
        // that interval.
        let backtrace = unsafe { Event::awaiter_backtrace_cell(event) };
        let inserted = self
            .backtraces
            .lock()
            .expect(NEVER_POISONED)
            .insert(backtrace);

        assert!(inserted, "an event was registered twice");
    }

    /// Unregisters an event immediately before its storage is released.
    ///
    /// # Safety
    ///
    /// `event` must be the initialized event passed to the matching [`register()`][Self::register]
    /// call. It must have remained alive at the same address and reachable only through shared
    /// references since registration, and those conditions must hold until this method returns.
    pub(crate) unsafe fn unregister<T: Send + 'static>(
        &self,
        event: NonNull<UnsafeCell<Event<T>>>,
    ) {
        // SAFETY: The caller guarantees that this is the same initialized event at the stable
        // address registered earlier and that only shared references have existed or can exist
        // through this call.
        let backtrace = unsafe { Event::awaiter_backtrace_cell(event) };
        let removed = self
            .backtraces
            .lock()
            .expect(NEVER_POISONED)
            .remove(&backtrace);

        assert!(removed, "an event was released twice");
    }

    /// Snapshots all stored awaiter backtraces.
    #[must_use]
    pub(crate) fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        let backtraces = self.backtraces.lock().expect(NEVER_POISONED);
        let mut snapshots = Vec::with_capacity(backtraces.len());

        for &backtrace in backtraces.iter() {
            // SAFETY: Every pointer is registered from an initialized event and is removed while
            // holding this same registry lock before that event is released. Holding the lock
            // therefore keeps the cell alive for this borrow. Only shared references to the cell
            // exist, and its mutex governs mutation of the optional backtrace.
            let backtrace = unsafe { backtrace.as_ref() };
            let backtrace = backtrace.lock().expect(NEVER_POISONED);

            if let Some(backtrace) = backtrace.as_ref() {
                snapshots.push(Arc::clone(backtrace));
            }
        }

        snapshots
    }
}

// SAFETY: The registry's set is accessed only while holding its mutex. Registered pointers target
// mutex-protected backtrace cells in thread-safe events, remain valid until they are removed under
// the same registry lock, and are never used to reach the payload.
unsafe impl Send for EventRegistry {}
// SAFETY: The same mutex and pointer-lifetime invariants make every shared operation synchronized.
unsafe impl Sync for EventRegistry {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl fmt::Debug for EventRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("backtraces", &self.backtraces)
            .finish()
    }
}

/// Tracks diagnostics for local events that have not yet been released.
///
/// This is the single-threaded counterpart of [`EventRegistry`]. Its registry and every backtrace
/// cell remain confined to the thread that owns the corresponding local pool or lake.
pub(crate) struct LocalEventRegistry {
    backtraces: RefCell<HashSet<NonNull<LocalBacktraceCell>>>,
}

impl LocalEventRegistry {
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            backtraces: RefCell::new(HashSet::new()),
        }
    }

    /// Registers a live event.
    ///
    /// # Safety
    ///
    /// `event` must point to an initialized event at a stable address. The event must remain alive
    /// at that address, and only shared references to it may be created, until the matching
    /// [`unregister()`][Self::unregister] call returns.
    pub(crate) unsafe fn register<T: 'static>(&self, event: NonNull<UnsafeCell<LocalEvent<T>>>) {
        // SAFETY: The caller guarantees that `event` is initialized at a stable address, remains
        // live there until unregistration, and is reached only through shared references throughout
        // that interval.
        let backtrace = unsafe { LocalEvent::awaiter_backtrace_cell(event) };
        let inserted = self.backtraces.borrow_mut().insert(backtrace);

        assert!(inserted, "an event was registered twice");
    }

    /// Unregisters an event immediately before its storage is released.
    ///
    /// # Safety
    ///
    /// `event` must be the initialized event passed to the matching [`register()`][Self::register]
    /// call. It must have remained alive at the same address and reachable only through shared
    /// references since registration, and those conditions must hold until this method returns.
    pub(crate) unsafe fn unregister<T: 'static>(&self, event: NonNull<UnsafeCell<LocalEvent<T>>>) {
        // SAFETY: The caller guarantees that this is the same initialized event at the stable
        // address registered earlier and that only shared references have existed or can exist
        // through this call.
        let backtrace = unsafe { LocalEvent::awaiter_backtrace_cell(event) };
        let removed = self.backtraces.borrow_mut().remove(&backtrace);

        assert!(removed, "an event was released twice");
    }

    /// Snapshots all stored awaiter backtraces.
    #[must_use]
    pub(crate) fn awaiter_backtraces(&self) -> Vec<Arc<Backtrace>> {
        let backtraces = self.backtraces.borrow();
        let mut snapshots = Vec::with_capacity(backtraces.len());

        for &backtrace in backtraces.iter() {
            // SAFETY: Every pointer is registered from an initialized event and is removed while
            // holding this same registry borrow before that event is released. Holding the borrow
            // therefore keeps the cell alive for this shared reference. The local event and
            // registry are thread-confined, and the cell mediates all mutation of the value.
            let backtrace = unsafe { backtrace.as_ref() };
            let backtrace = backtrace.borrow();

            if let Some(backtrace) = backtrace.as_ref() {
                snapshots.push(Arc::clone(backtrace));
            }
        }

        snapshots
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl fmt::Debug for LocalEventRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("backtraces", &self.backtraces)
            .finish()
    }
}
