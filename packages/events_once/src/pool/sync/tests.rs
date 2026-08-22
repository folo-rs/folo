#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::Barrier;
use std::task::{self, Poll, Waker};
use std::{iter, thread};

use futures::executor::block_on;
use static_assertions::assert_impl_all;
#[cfg(debug_assertions)]
use testing::assert_panics_with;
use testing::with_watchdog;

use super::*;
#[cfg(debug_assertions)]
use crate::assert_inspect_awaiters_is_reentrant;
use crate::{
    Disconnected, PanickingPayload, assert_disconnected_send_payload_panic_releases_event,
    assert_receiver_waker_panic_handoff_releases_event, assert_unread_payload_panic_releases_event,
};

// The payload satisfies only the bound that the pool's API requires (`Send`) and lacks every
// trait asserted here, so each of them is supplied by the pool's own synchronization and
// storage rather than inherited from the payload. A trait object payload also has to preserve
// the thread-safety traits (regression test for #142).
assert_impl_all!(EventPool<Box<dyn Send>>: Send, Sync, UnwindSafe, RefUnwindSafe);

#[test]
fn disconnected_send_payload_panic_releases_event() {
    let pool = EventPool::<PanickingPayload>::new();

    assert_disconnected_send_payload_panic_releases_event(
        || pool.rent(),
        PooledSender::send,
        || pool.is_empty(),
    );
}

#[test]
fn receiver_waker_panic_handoff_releases_event() {
    let pool = EventPool::<i32>::new();

    assert_receiver_waker_panic_handoff_releases_event(
        || pool.rent(),
        PooledSender::send,
        || pool.is_empty(),
    );
}

#[test]
fn unread_payload_panic_releases_event() {
    let pool = EventPool::<PanickingPayload>::new();

    assert_unread_payload_panic_releases_event(
        || pool.rent(),
        PooledSender::send,
        || pool.is_empty(),
    );
}

mod concurrency;
mod diagnostics;
mod lifecycle;
