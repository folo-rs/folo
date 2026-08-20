use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::LocalEvent;

/// Enables a sender or receiver to reference the event that connects them.
///
/// An implementation owns the storage strategy of one event: it turns the storage into the
/// shared reference that every event operation goes through, and it releases that storage once
/// the state machine has handed cleanup responsibility to a single endpoint.
///
/// Implementations are plain handles that carry no pinning requirements of their own, hence the
/// `Unpin` bound, which is what lets a pinned endpoint be projected to its core without unsafe
/// code even when the payload is `!Unpin`.
///
/// # Safety
///
/// An implementation must guarantee that:
///
/// * `Deref::deref()` returns a reference to one specific event that is initialized, aligned
///   and located at a stable address, and that stays valid for as long as any endpoint holding
///   the reference can access it.
/// * No exclusive reference to that event exists for that duration, so the only aliasing is
///   between shared references, which is what the event's `UnsafeCell` interior mutability
///   requires.
/// * `release_event()` releases the storage in whatever manner the storage strategy requires
///   and does not access the event afterwards.
pub(crate) unsafe trait LocalRef<T>:
    Deref<Target = UnsafeCell<LocalEvent<T>>> + fmt::Debug + Unpin
{
    /// Releases the event.
    ///
    /// # Safety
    ///
    /// The caller must have observed the terminal state transition that granted it sole
    /// responsibility for cleaning up the event (see `state.rs`), and must not access the event
    /// through this or any other reference afterwards.
    unsafe fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
///
/// The storage belongs to whoever placed the event into it, so releasing the event here does not
/// release any memory.
pub(crate) struct PtrLocalRef<T> {
    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> PtrLocalRef<T> {
    /// Creates a reference to an event that lives in storage owned by someone else.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the pointer references an initialized event that remains
    /// valid, remains at a stable address and is only ever accessed through shared references,
    /// for as long as any endpoint created from this reference can access it.
    #[must_use]
    pub(crate) unsafe fn new(event: NonNull<UnsafeCell<LocalEvent<T>>>) -> Self {
        Self { event }
    }
}

// SAFETY: The pointer comes from the caller of `new()`, whose contract is exactly the event
// identity, validity, stable address and shared-only aliasing that this trait requires.
// Releasing the event releases no storage (the placer owns it) and only clears the event's own
// diagnostic state before returning.
unsafe impl<T: 'static> LocalRef<T> for PtrLocalRef<T> {
    #[inline]
    unsafe fn release_event(&self) {
        // The storage is owned by whoever placed the event there and is reused without dropping
        // the event, so we clear its diagnostic state before we let go of it.
        #[cfg(debug_assertions)]
        LocalEvent::clear_awaiter_backtrace(self);
    }
}

impl<T: 'static> Deref for PtrLocalRef<T> {
    type Target = UnsafeCell<LocalEvent<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The creator of this reference guaranteed that the event is initialized and
        // stays at this address for as long as any endpoint can reach it, and that it is only
        // ever accessed through shared references, so no exclusive reference can alias this one.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for PtrLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

/// References an event stored on the heap.
///
/// The two references returned by [`new_pair()`][Self::new_pair] share one allocation, which the
/// last endpoint to release the event frees.
pub(crate) struct BoxedLocalRef<T> {
    event: NonNull<UnsafeCell<LocalEvent<T>>>,
}

impl<T: 'static> BoxedLocalRef<T> {
    #[must_use]
    pub(crate) fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit = unsafe {
            event
                .cast::<UnsafeCell<MaybeUninit<LocalEvent<T>>>>()
                .as_mut()
        };

        LocalEvent::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<LocalEvent<T>>()
    }
}

// SAFETY: The allocation is made here, for exactly one event that is initialized before any
// reference to it escapes, and a heap allocation never moves, so both references identify the
// same valid event at a stable address for as long as an endpoint holds one. The only references
// handed out are the shared ones from `deref()`. Releasing frees that allocation once and does
// not touch the event afterwards.
unsafe impl<T: 'static> LocalRef<T> for BoxedLocalRef<T> {
    unsafe fn release_event(&self) {
        // The caller tells us that they are the last endpoint, so nothing else can possibly
        // be accessing the event any more. We can safely release the memory.

        // Releasing the memory does not drop the event, so we clear its diagnostic state first.
        #[cfg(debug_assertions)]
        LocalEvent::clear_awaiter_backtrace(self);

        // SAFETY: The pointer and layout are the ones from the matching `alloc()` in
        // `new_pair()`, and the caller of `release_event()` guaranteed that the state machine
        // selected them as the only endpoint responsible for cleanup, so this is the only
        // release of this allocation.
        unsafe {
            dealloc(self.event.as_ptr().cast(), Self::layout());
        }
    }
}

impl<T: 'static> Deref for BoxedLocalRef<T> {
    type Target = UnsafeCell<LocalEvent<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: The event was initialized into this allocation in `new_pair()` and the
        // allocation is only freed once an endpoint is told it is the last one, so it is valid
        // here. We only ever hand out shared references to it, so no exclusive reference aliases.
        unsafe { self.event.as_ref() }
    }
}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: 'static> fmt::Debug for BoxedLocalRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::assert_not_impl_any;

    use super::*;

    assert_not_impl_any!(BoxedLocalRef<i32>: Send, Sync);
    assert_not_impl_any!(PtrLocalRef<i32>: Send, Sync);
}
