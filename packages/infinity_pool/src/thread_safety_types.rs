//! We have various assertions about thread safety elsewhere in the crate.
//!
//! To simplify these assertions, we define type aliases for common thread
//! safety related types here.

#![allow(
    dead_code,
    reason = "old toolchains cannot identify that these aliases are used in static assertions"
)]

use std::cell::Cell;
use std::marker::PhantomData;

use static_assertions::{assert_impl_all, assert_not_impl_any};

pub(crate) type SendAndSync = String;
assert_impl_all!(SendAndSync: Send, Sync);

pub(crate) type NotSendNotSync = *const ();
assert_not_impl_any!(NotSendNotSync: Send, Sync);

pub(crate) type SendNotSync = Cell<()>;
assert_impl_all!(SendNotSync: Send);
assert_not_impl_any!(SendNotSync: Sync);

pub(crate) struct NotSendSync {
    _single_threaded_by_default: PhantomData<*const ()>,
}

// SAFETY: Just for testing.
unsafe impl Sync for NotSendSync {}

assert_impl_all!(NotSendSync: Sync);
assert_not_impl_any!(NotSendSync: Send);
