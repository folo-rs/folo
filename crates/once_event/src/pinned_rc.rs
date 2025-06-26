use std::cell::{Cell, RefCell};
use std::pin::Pin;
use std::rc::Rc;

use pinned_pool::{DropPolicy, Key, PinnedPool};

/// Type alias for a `PinnedPool` with the right type to contain a `PinnedRcBox` holding a T, with
/// an outer `RefCell` that performs runtime borrow checking for safety. This is the backing storage
/// for reference-counting `PinnedRc` smart pointers.
///
/// This storage is compatible with all types of `PinnedRc` smart pointers, though you may need to wrap
/// it in some extra layers they you can call the desired `insert_into_*()` method on it:
///
/// * `Rc<PinnedRcStorage<T>>` if you want to use `RcPinnedRc`.
/// * `Pin<Box<PinnedRcStorage<T>>>>` if you want to use `UnsafePinnedRc`.
///
/// There is also a shorthand function for creating a new slab chain with this type, specialized
/// for the different kinds of smart pointers:
/// * `PinnedRcBox<T>::new_storage_ref()`
/// * `PinnedRcBox<T>::new_storage_rc()`
/// * `PinnedRcBox<T>::new_storage_unsafe()`
pub(crate) type PinnedRcStorage<T> = RefCell<PinnedPool<PinnedRcBox<T>>>;

/// Can be used as the item type in a pinned slab chain to transform it into a reference-counting
/// slab chain, where an item is removed from the chain when the last reference to it is dropped.
///
/// This is an opaque type whose utility ends after it has been inserted into a slab chain. Insert
/// the item via `.insert_into()` and thereafter access it via the `PinnedRc` you obtain from this.
///
/// There are different forms of `PinnedRc` that can be created to point at this item, differing by the
/// way in which they reference the slab itself:
///
/// * `RefPinnedRc` maintains a reference to the slab chain, which means the slab chain is borrowed
///   for as long as any smart pointer into it is alive. Simple for lifetime management but you
///   will need to add lifetime annotations EVERYWHERE you use the smart pointers.
/// * `RcPinnedRc` maintains a reference to the slab chain via another `Rc`, which removes the need to
///   track lifetimes but incurs extra reference counting cost for each operation (which may be
///   negligible).
/// * `UnsafePinnedRc` maintains a reference to a the slab chain using a raw pointer. Obviously rather
///   unsafe to use and requires the slab chain itself to be pinned but if you can guarantee that no
///   smart pointer will ever be alive after the slab chain is dropped, this is essentially free of
///   any runtime overhead.
#[derive(Debug)]
#[expect(
    clippy::module_name_repetitions,
    reason = "PinnedRc prefix is appropriate for the type hierarchy"
)]
pub struct PinnedRcBox<T> {
    value: T,
    ref_count: Cell<usize>,
}

impl<T> PinnedRcBox<T> {
    pub(crate) fn new(value: T) -> Self {
        Self {
            value,
            ref_count: Cell::new(0),
        }
    }

    /// Inserts the boxed value into a slab chain that will be referenced via direct reference.
    ///
    /// You can easily allocate such a slab chain via `new_storage_ref()`.
    pub(crate) fn insert_into_ref(
        self,
        slab_chain: &RefCell<PinnedPool<Self>>,
    ) -> RefPinnedRc<'_, T> {
        let mut slab_chain_mut = slab_chain.borrow_mut();
        let inserter = slab_chain_mut.begin_insert();
        let key = inserter.key();

        // We are creating the first reference here, embodied in the first PinnedRc we return.
        self.ref_count.set(1);

        // In principle, someone could go around removing arbitrary items from the slab chain and
        // cause memory corruption. However, we do not consider that in scope of our safety model
        // because we are not even exposing the key, so the only attack is to guess the key,
        // which is a sufficient low probability event to happen by accident that it is not worth
        // thinking about (and not worth adding comments about if we chose to mark this unsafe).
        let value = inserter.insert(self);

        RefPinnedRc {
            slab_chain,
            // SAFETY: The risk is that we un-pin something !Unpin. We do not do that - all pinned
            // slab items are forever pinned and we always expose them as pinned pointers.
            value: std::ptr::from_ref(unsafe { Pin::into_inner_unchecked(value) }),
            key,
        }
    }

    /// Inserts the boxed value into a slab chain that will be referenced via `Rc`.
    ///
    /// You can easily allocate such a slab chain via `new_storage_rc()`.
    pub(crate) fn insert_into_rc(self, slab_chain: Rc<RefCell<PinnedPool<Self>>>) -> RcPinnedRc<T> {
        let (key, value) = {
            let mut slab_chain_mut = slab_chain.borrow_mut();
            let inserter = slab_chain_mut.begin_insert();
            let key = inserter.key();

            // We are creating the first reference here, embodied in the first PinnedRc we return.
            self.ref_count.set(1);

            // In principle, someone could go around removing arbitrary items from the slab chain and
            // cause memory corruption. However, we do not consider that in scope of our safety model
            // because we are not even exposing the key, so the only attack is to guess the key,
            // which is a sufficient low probability event to happen by accident that it is not worth
            // thinking about (and not worth adding comments about if we chose to mark this unsafe).
            let value = inserter.insert(self);

            // SAFETY: The risk is that we un-pin something !Unpin. We do not do that - all pinned
            // slab items are forever pinned and we always expose them via `Pin`.
            let value = std::ptr::from_ref(unsafe { Pin::into_inner_unchecked(value) });

            (key, value)
        };

        RcPinnedRc {
            slab_chain,
            key,
            value,
        }
    }

    /// Inserts the boxed value into a slab chain that will be referenced via a raw pointer.
    ///
    /// You can easily allocate such a slab chain via `new_storage_unsafe()`.
    ///
    /// # Safety
    ///
    /// The caller must guarantee that the slab chain outlives every box inserted into it.
    pub(crate) unsafe fn insert_into_unsafe(
        self,
        slab_chain: Pin<&RefCell<PinnedPool<Self>>>,
    ) -> UnsafePinnedRc<T> {
        let (key, value) = {
            let mut slab_chain_mut = slab_chain.borrow_mut();
            let inserter = slab_chain_mut.begin_insert();
            let key = inserter.key();

            // We are creating the first reference here, embodied in the first PinnedRc we return.
            self.ref_count.set(1);

            // In principle, someone could go around removing arbitrary items from the slab chain and
            // cause memory corruption. However, we do not consider that in scope of our safety model
            // because we are not even exposing the key, so the only attack is to guess the key,
            // which is a sufficient low probability event to happen by accident that it is not worth
            // thinking about (and not worth adding comments about if we chose to mark this unsafe).
            let value = inserter.insert(self);

            // SAFETY: The risk is that we un-pin something !Unpin. We do not do that - all pinned
            // slab items are forever pinned and we always expose them as pinned pointers.
            let value = std::ptr::from_ref(unsafe { Pin::into_inner_unchecked(value) });

            (key, value)
        };

        UnsafePinnedRc {
            // SAFETY: The caller is responsible for ensuring the slab chain outlives us.
            slab_chain: std::ptr::from_ref(unsafe { Pin::into_inner_unchecked(slab_chain) }),
            value,
            key,
        }
    }

    /// Allocates a new `PinnedRc` storage intended for use with `insert_into_ref()`.
    ///
    /// # Panics
    ///
    /// All `PinnedRc` values must be dropped by the time the storage is dropped or it will panic.
    pub(crate) fn new_storage_ref() -> RefCell<PinnedPool<Self>> {
        // We configure "must not drop items" policy because if all the PinnedRcs are holding
        // references to the slab then they should be cleaning up items when the PinnedRcs are
        // dropped. Therefore, if something still exists in the slab chain afterwards, something
        // went very wrong and we need to raise the alarm.
        RefCell::new(
            PinnedPool::builder()
                .drop_policy(DropPolicy::MustNotDropItems)
                .build(),
        )
    }

    /// Allocates a new `PinnedRc` storage intended for use with `insert_into_rc()`.
    ///
    /// # Panics
    ///
    /// All `PinnedRc` values must be dropped by the time the storage is dropped or it will panic.
    #[allow(dead_code, reason = "May be useful for future extensions")]
    pub(crate) fn new_storage_rc() -> Rc<RefCell<PinnedPool<Self>>> {
        // We configure "must not drop items" policy because if all the PinnedRcs are holding
        // references to the slab via Rc then it should be impossible for the slab chain to drop
        // first because the references from its own items should be holding it alive.
        Rc::new(RefCell::new(
            PinnedPool::builder()
                .drop_policy(DropPolicy::MustNotDropItems)
                .build(),
        ))
    }

    /// Allocates a new `PinnedRc` storage intended for use with `insert_into_unsafe()`.
    ///
    /// # Panics
    ///
    /// All `PinnedRc` values must be dropped by the time the storage is dropped or it will panic.
    #[allow(dead_code, reason = "May be useful for future extensions")]
    pub(crate) fn new_storage_unsafe() -> Pin<Box<RefCell<PinnedPool<Self>>>> {
        // It is the responsibility of the caller to ensure that the slab chain outlives all the
        // smart pointers that point into it. Dropping the slab chain while there are still items
        // in it here indicates that the caller failed to perform their duty.
        Box::pin(RefCell::new(
            PinnedPool::builder()
                .drop_policy(DropPolicy::MustNotDropItems)
                .build(),
        ))
    }
}

impl<T> From<T> for PinnedRcBox<T> {
    fn from(value: T) -> Self {
        Self::new(value)
    }
}

// ################## RefPinnedRc ################## //

/// A reference-counting smart pointer to an item stored in a `PinnedPool<PinnedRcBox<T>>`.
///
/// You can get a pinned reference to the item via `deref_pin()` and you can clone the smart
/// pointer and that's about it.
///
/// # Panics
///
/// Dropping a `PinnedRc` may take an exclusive reference on the slab chain via runtime borrow
/// checking. Make sure you are not holding any references to the slab chain yourself when dropping
/// any `PinnedRc` values or the drop will panic.
#[derive(Debug)]
#[expect(
    clippy::module_name_repetitions,
    reason = "RefPinnedRc name clearly indicates its purpose"
)]
pub struct RefPinnedRc<'slab, T> {
    // We may need to mutate the chain at any time, so we require it to be in a RefCell.
    slab_chain: &'slab RefCell<PinnedPool<PinnedRcBox<T>>>,

    key: Key,

    // We ourselves are keeping this value alive, so we do not take a reference to it but rather
    // store it directly as a pointer that we can turn into an appropriately-lifetimed reference
    // on demand.
    value: *const PinnedRcBox<T>,
}

impl<T> RefPinnedRc<'_, T> {
    /// Returns a pinned reference to the value stored in the slab.
    ///
    /// This method provides safe access to the underlying value while maintaining
    /// the pinning guarantees required by the `PinnedPool`.
    #[must_use]
    pub fn deref_pin(&self) -> Pin<&T> {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value_ref = unsafe { &*self.value };
        // SAFETY: The value we point to is guaranteed pinned, so we are not at risk of unpinning anything.
        unsafe { Pin::new_unchecked(&value_ref.value) }
    }
}

impl<T> Clone for RefPinnedRc<'_, T> {
    fn clone(&self) -> Self {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value = unsafe { &*self.value };
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Reference counting increment cannot overflow in practice"
        )]
        value.ref_count.set(value.ref_count.get() + 1);

        Self {
            slab_chain: self.slab_chain,
            value: self.value,
            key: self.key,
        }
    }
}

impl<T> Drop for RefPinnedRc<'_, T> {
    fn drop(&mut self) {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let ref_count = unsafe { &*self.value }.ref_count.get();

        assert!(ref_count > 0);

        if ref_count == 1 {
            self.slab_chain.borrow_mut().remove(self.key);
            // `value` points to invalid memory now, which is allowed for raw pointers.
            // There is no regular reference to `value` existing in this branch.
        } else {
            // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "Reference counting decrement cannot underflow due to guard above"
            )]
            unsafe { &*self.value }.ref_count.set(ref_count - 1);
        }
    }
}

// ################## RcPinnedRc ################## //

/// A reference-counting smart pointer to an item stored in a `PinnedPool<PinnedRcBox<T>>`.
///
/// You can get a pinned reference to the item via `deref_pin()` and you can clone the smart
/// pointer and that's about it.
///
/// # Panics
///
/// Dropping a `PinnedRc` may take an exclusive reference on the slab chain via runtime borrow
/// checking. Make sure you are not holding any references to the slab chain yourself when dropping
/// any `PinnedRc` values or the drop will panic.
#[derive(Debug)]
#[expect(
    clippy::module_name_repetitions,
    reason = "RcPinnedRc name clearly indicates its purpose"
)]
pub struct RcPinnedRc<T> {
    // We may need to mutate the chain at any time, so we require it to be in a RefCell.
    slab_chain: Rc<RefCell<PinnedPool<PinnedRcBox<T>>>>,

    key: Key,

    // We ourselves are keeping this value alive, so we do not take a reference to it but rather
    // store it directly as a pointer that we can turn into an appropriately-lifetimed reference
    // on demand.
    value: *const PinnedRcBox<T>,
}

impl<T> RcPinnedRc<T> {
    pub(crate) fn deref_pin(&self) -> Pin<&T> {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value_ref = unsafe { &*self.value };
        // SAFETY: The value we point to is guaranteed pinned, so we are not at risk of unpinning anything.
        unsafe { Pin::new_unchecked(&value_ref.value) }
    }
}

impl<T> Clone for RcPinnedRc<T> {
    fn clone(&self) -> Self {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value = unsafe { &*self.value };
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Reference counting increment cannot overflow in practice"
        )]
        value.ref_count.set(value.ref_count.get() + 1);

        Self {
            slab_chain: Rc::clone(&self.slab_chain),
            value: self.value,
            key: self.key,
        }
    }
}

impl<T> Drop for RcPinnedRc<T> {
    fn drop(&mut self) {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let ref_count = unsafe { &*self.value }.ref_count.get();

        assert!(ref_count > 0);

        if ref_count == 1 {
            self.slab_chain.borrow_mut().remove(self.key);
            // `value` points to invalid memory now, which is allowed for raw pointers.
            // There is no regular reference to `value` existing in this branch.
        } else {
            // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "Reference counting decrement cannot underflow due to guard above"
            )]
            unsafe { &*self.value }.ref_count.set(ref_count - 1);
        }
    }
}

// ################## UnsafePinnedRc ################## //

/// A reference-counting smart pointer to an item stored in a `PinnedPool<PinnedRcBox<T>>`.
///
/// You can get a pinned reference to the item via `deref_pin()` and you can clone the smart
/// pointer and that's about it.
///
/// # Safety
///
/// This smart pointer maintains a raw reference to the underlying slab chain. The caller is
/// responsible for ensuring that the lifetime of the slab chain exceeds the lifetime of every
/// smart pointer into the slab chain.
///
/// # Panics
///
/// Dropping a `PinnedRc` may take an exclusive reference on the slab chain via runtime borrow
/// checking. Make sure you are not holding any references to the slab chain yourself when dropping
/// any `PinnedRc` values or the drop will panic.
#[derive(Debug)]
#[expect(
    clippy::module_name_repetitions,
    reason = "UnsafePinnedRc name clearly indicates its purpose"
)]
pub struct UnsafePinnedRc<T> {
    // We may need to mutate the chain at any time, so we require it to be in a RefCell.
    // The caller is responsible for ensuring this outlives us.
    slab_chain: *const RefCell<PinnedPool<PinnedRcBox<T>>>,

    key: Key,

    // We ourselves are keeping this value alive, so we do not take a reference to it but rather
    // store it directly as a pointer that we can turn into an appropriately-lifetimed reference
    // on demand.
    value: *const PinnedRcBox<T>,
}

impl<T> UnsafePinnedRc<T> {
    pub(crate) fn deref_pin(&self) -> Pin<&T> {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value_ref = unsafe { &*self.value };
        // SAFETY: The value we point to is guaranteed pinned, so we are not at risk of unpinning anything.
        unsafe { Pin::new_unchecked(&value_ref.value) }
    }
}

impl<T> Clone for UnsafePinnedRc<T> {
    fn clone(&self) -> Self {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let value = unsafe { &*self.value };
        #[expect(
            clippy::arithmetic_side_effects,
            reason = "Reference counting increment cannot overflow in practice"
        )]
        value.ref_count.set(value.ref_count.get() + 1);

        Self {
            slab_chain: self.slab_chain,
            value: self.value,
            key: self.key,
        }
    }
}

impl<T> Drop for UnsafePinnedRc<T> {
    fn drop(&mut self) {
        // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
        let ref_count = unsafe { &*self.value }.ref_count.get();

        assert!(ref_count > 0);

        if ref_count == 1 {
            // SAFETY: The caller is responsible for ensuring the slab chain outlives us.
            let slab_chain = unsafe { &*self.slab_chain };
            slab_chain.borrow_mut().remove(self.key);
            // `value` points to invalid memory now, which is allowed for raw pointers.
            // There is no regular reference to `value` existing in this branch.
        } else {
            // SAFETY: We are the thing keeping the `value` pointer alive, so this is safe.
            #[expect(
                clippy::arithmetic_side_effects,
                reason = "Reference counting decrement cannot underflow due to guard above"
            )]
            unsafe { &*self.value }.ref_count.set(ref_count - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn ref_smoke_test() {
        let storage = PinnedRcBox::<usize>::new_storage_ref();

        let item = PinnedRcBox::new(42).insert_into_ref(&storage);
        assert_eq!(*item.deref_pin(), 42);

        let item_clone = RefPinnedRc::clone(&item);
        assert_eq!(*item_clone.deref_pin(), 42);

        drop(item);
    }

    #[test]
    fn ref_value_is_dropped_after_last_rc_drop() {
        // While we do not exactly have a way to introspect a slab chain, we can do our own checks
        // by holding a weak reference and seeing if the weak reference becomes dead when the last
        // strong reference is dropped via the PinnedRc.

        let canary = Arc::new(55);
        let canary_weak = Arc::downgrade(&canary);

        let storage = PinnedRcBox::<Arc<usize>>::new_storage_ref();

        let item = PinnedRcBox::new(canary).insert_into_ref(&storage);
        assert_eq!(**item.deref_pin(), 55);

        let item_clone = RefPinnedRc::clone(&item);
        assert_eq!(**item_clone.deref_pin(), 55);

        drop(item);
        drop(item_clone);

        assert!(canary_weak.upgrade().is_none());
    }

    #[test]
    fn rc_smoke_test() {
        let storage = PinnedRcBox::<usize>::new_storage_rc();

        let item = PinnedRcBox::new(42).insert_into_rc(Rc::clone(&storage));
        assert_eq!(*item.deref_pin(), 42);

        let item_clone = RcPinnedRc::clone(&item);
        assert_eq!(*item_clone.deref_pin(), 42);

        drop(item);
    }

    #[test]
    fn rc_value_is_dropped_after_last_rc_drop() {
        // While we do not exactly have a way to introspect a slab chain, we can do our own checks
        // by holding a weak reference and seeing if the weak reference becomes dead when the last
        // strong reference is dropped via the PinnedRc.

        let canary = Arc::new(55);
        let canary_weak = Arc::downgrade(&canary);

        let storage = PinnedRcBox::<Arc<usize>>::new_storage_rc();

        let item = PinnedRcBox::new(canary).insert_into_rc(Rc::clone(&storage));
        assert_eq!(**item.deref_pin(), 55);

        let item_clone = RcPinnedRc::clone(&item);
        assert_eq!(**item_clone.deref_pin(), 55);

        drop(item);
        drop(item_clone);

        assert!(canary_weak.upgrade().is_none());
    }

    #[test]
    fn unsafe_smoke_test() {
        let storage = PinnedRcBox::<usize>::new_storage_unsafe();

        // SAFETY: We are responsible for ensuring the slab chain outlives all the smart pointers.
        // In this case, they both are dropped in the same function, so life is easy. At other
        // times, it may not be so easy!
        let item = unsafe { PinnedRcBox::new(42).insert_into_unsafe(storage.as_ref()) };
        assert_eq!(*item.deref_pin(), 42);

        let item_clone = UnsafePinnedRc::clone(&item);
        assert_eq!(*item_clone.deref_pin(), 42);

        drop(item);
    }

    #[test]
    fn unsafe_value_is_dropped_after_last_rc_drop() {
        // While we do not exactly have a way to introspect a slab chain, we can do our own checks
        // by holding a weak reference and seeing if the weak reference becomes dead when the last
        // strong reference is dropped via the PinnedRc.

        let canary = Arc::new(55);
        let canary_weak = Arc::downgrade(&canary);

        let storage = PinnedRcBox::<Arc<usize>>::new_storage_unsafe();

        // SAFETY: We are responsible for ensuring the slab chain outlives all the smart pointers.
        // In this case, they both are dropped in the same function, so life is easy. At other
        // times, it may not be so easy!
        let item = unsafe { PinnedRcBox::new(canary).insert_into_unsafe(storage.as_ref()) };
        assert_eq!(**item.deref_pin(), 55);

        let item_clone = UnsafePinnedRc::clone(&item);
        assert_eq!(**item_clone.deref_pin(), 55);

        drop(item);
        drop(item_clone);

        assert!(canary_weak.upgrade().is_none());
    }
}
