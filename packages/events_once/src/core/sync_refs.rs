use std::alloc::{Layout, alloc, dealloc};
use std::any::type_name;
use std::cell::UnsafeCell;
use std::fmt;
use std::mem::MaybeUninit;
use std::ops::Deref;
use std::ptr::NonNull;

use crate::Event;

/// Enables a sender or receiver to reference the event that connects them.
///
/// An implementation is the storage policy of one endpoint: it turns ownership of the event
/// storage into the shared references that every event operation uses, and it performs the
/// storage-specific release once the state machine has granted that endpoint cleanup ownership
/// (see `state.rs`). Both obligations are memory-safety critical, which is why they are stated
/// here instead of being rediscovered by each storage strategy and each generic core.
///
/// # Safety
///
/// An implementation must guarantee, for as long as an endpoint holding the reference can
/// access the event:
///
/// * Dereferencing yields one specific event that is initialized, properly aligned and valid
///   for the entire lifetime of the returned reference.
/// * The event is reachable only through shared references, so that the concurrent access of
///   the two endpoints is the only aliasing that can occur.
/// * The storage backing the event is neither released nor handed to another event except
///   through [`release_event`][Self::release_event].
///
/// A single implementation type may be held by both endpoints, in which case these guarantees
/// must hold for each of them independently.
///
/// The `Unpin` bound records that an endpoint reference is a pointer handle and never
/// structurally pinned, which is what lets the generic endpoint cores project `Pin<&mut Self>`
/// without unsafe code.
pub(crate) unsafe trait EventRef<T>:
    Deref<Target = UnsafeCell<Event<T>>> + Unpin + fmt::Debug
{
    /// Releases the event, returning its storage to whoever provides it.
    ///
    /// # Safety
    ///
    /// The caller must have performed the state machine transition that assigned it sole
    /// cleanup ownership of the event (see `state.rs`), and must not access the event
    /// afterwards, neither through this reference nor through any other.
    unsafe fn release_event(&self);
}

/// References an event stored anywhere, via raw pointer.
pub(crate) struct PtrRef<T> {
    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T: Send + 'static> PtrRef<T> {
    /// # Safety
    ///
    /// The caller must guarantee that the pointer references an initialized event in storage
    /// that remains allocated, pinned and unused by any other event until both endpoints have
    /// released it, and that nothing accesses that storage through an exclusive reference while
    /// the endpoints exist.
    #[must_use]
    pub(crate) unsafe fn new(event: NonNull<UnsafeCell<Event<T>>>) -> Self {
        Self { event }
    }
}

// SAFETY: The storage is provided by the caller of `PtrRef::new`, whose contract keeps one
// initialized, aligned event in place and unused by any other event until both endpoints have
// released it. The pointer is only ever turned into shared references, and `release_event`
// leaves the storage to its owner, so no release can invalidate a reference that an endpoint
// still holds.
unsafe impl<T> EventRef<T> for PtrRef<T>
where
    T: Send + 'static,
{
    #[inline]
    unsafe fn release_event(&self) {
        // The storage is owned by whoever placed the event there and is reused without dropping
        // the event, so we clear its diagnostic state before we let go of it.
        #[cfg(debug_assertions)]
        Event::clear_awaiter_backtrace(self);
    }
}

impl<T> Deref for PtrRef<T>
where
    T: Send + 'static,
{
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Validity: the `PtrRef::new` contract guarantees an initialized, aligned event
        // that outlives every endpoint holding this reference. Aliasing: we only ever hand out
        // shared references to the event, and the event synchronizes access to its interior
        // fields itself, so no exclusive reference to it is ever created.
        unsafe { self.event.as_ref() }
    }
}

// SAFETY: Moving this reference to another thread is sound because:
// * The pointer remains valid there: the `PtrRef::new` contract binds the storage to the
//   lifetime of the endpoints rather than to the thread that created them.
// * Shared access from another thread is permitted: `Event<T>` is `Sync` when `T: Send`, and
//   this type only ever produces shared references to the event.
// * No release can race that access: this type does not own the storage, so `release_event`
//   only clears diagnostic state that the event itself synchronizes.
//
// Only `T: Send` is stated. The `'static` bound that the constructor and the `EventRef`
// implementation require is deliberately not repeated here, because repeating it triggers a
// rustc bug (rust-lang/rust#110338) in async generator Send inference with trait object type
// params. The proof above does not rely on it.
unsafe impl<T: Send> Send for PtrRef<T> {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for PtrRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

/// References an event stored on the heap.
pub(crate) struct BoxedRef<T> {
    event: NonNull<UnsafeCell<Event<T>>>,
}

impl<T> BoxedRef<T>
where
    T: Send + 'static,
{
    #[must_use]
    pub(crate) fn new_pair() -> (Self, Self) {
        // SAFETY: The layout is correct for the type we are using - all is well.
        let event = NonNull::new(unsafe { alloc(Self::layout()) })
            .expect("memory allocation failed - fatal error")
            .cast();

        // SAFETY: MaybeUninit is a transparent wrapper, so the layout matches.
        // This is the only reference, so we have exclusive access rights.
        let event_as_maybe_uninit =
            unsafe { event.cast::<UnsafeCell<MaybeUninit<Event<T>>>>().as_mut() };

        Event::new_in_inner(event_as_maybe_uninit);

        (Self { event }, Self { event })
    }

    const fn layout() -> Layout {
        Layout::new::<Event<T>>()
    }
}

// SAFETY: `new_pair` allocates one event with the correct layout, initializes it and hands one
// reference to each endpoint, so both references identify the same initialized, aligned event.
// The allocation is only ever turned into shared references, and it is freed by `release_event`,
// whose contract requires the caller to hold sole cleanup ownership - so the allocation outlives
// every reference that either endpoint can still use.
unsafe impl<T> EventRef<T> for BoxedRef<T>
where
    T: Send + 'static,
{
    unsafe fn release_event(&self) {
        // Releasing the memory does not drop the event, so we clear its diagnostic state first.
        #[cfg(debug_assertions)]
        Event::clear_awaiter_backtrace(self);

        // SAFETY: The pointer comes from `new_pair`, which allocated it with this exact layout.
        // The caller guarantees sole cleanup ownership, so nothing can access the event during
        // or after this call and no second release of the same allocation can occur.
        unsafe {
            dealloc(self.event.as_ptr().cast(), Self::layout());
        }
    }
}

impl<T> Deref for BoxedRef<T>
where
    T: Send + 'static,
{
    type Target = UnsafeCell<Event<T>>;

    fn deref(&self) -> &Self::Target {
        // SAFETY: Validity: `new_pair` initialized the event in this allocation, and the
        // allocation is only freed by the endpoint that the state machine put in charge of
        // cleanup, which by then is the last endpoint. Aliasing: we only ever hand out shared
        // references to the event, and the event synchronizes access to its interior fields
        // itself, so no exclusive reference to it is ever created.
        unsafe { self.event.as_ref() }
    }
}

// SAFETY: Moving this reference to another thread is sound because:
// * The allocation remains valid there: it is created by `new_pair` and freed only via
//   `release_event`, whose contract requires the caller to be the endpoint that the state
//   machine put in charge of cleanup - by then the last endpoint, so no reference held by the
//   other endpoint can outlive the deallocation.
// * Shared access from another thread is permitted: `Event<T>` is `Sync` when `T: Send`, and
//   this type only ever produces shared references to the event.
//
// Only `T: Send` is stated. The `'static` bound that the constructor and the `EventRef`
// implementation require is deliberately not repeated here, because repeating it triggers a
// rustc bug (rust-lang/rust#110338) in async generator Send inference with trait object type
// params. The proof above does not rely on it.
unsafe impl<T: Send> Send for BoxedRef<T> {}

#[cfg_attr(coverage_nightly, coverage(off))] // No API contract to test.
impl<T: Send + 'static> fmt::Debug for BoxedRef<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(type_name::<Self>())
            .field("event", &self.event)
            .finish()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use static_assertions::{assert_impl_all, assert_not_impl_any};

    use super::*;

    // `Box<dyn Send>` is the minimally qualified payload for these types: it satisfies the
    // `T: Send + 'static` bound that the references require and lacks every other auto trait,
    // so a passing assertion cannot be explained by the payload's own markers.
    // Preserving `Send` for such payloads is also a regression test for #142.
    assert_impl_all!(BoxedRef<Box<dyn Send>>: Send);
    assert_not_impl_any!(BoxedRef<Box<dyn Send>>: Sync);

    assert_impl_all!(PtrRef<Box<dyn Send>>: Send);
    assert_not_impl_any!(PtrRef<Box<dyn Send>>: Sync);
}
