//! Cycle-accurate benchmarks for the `future_deque` crate.
//!
//! Paired with `future_deque.rs` (the Criterion benches for the same
//! types) which exercises the build-and-drain shape with mixed
//! activity ratios. The cycle benches isolate the per-call cost of
//! the hot operations so they can be tracked at instruction-level
//! granularity:
//!
//! * `push_back_empty` — `push_back` into an empty deque.
//! * `push_back_into_100` — `push_back` when 100 futures are already
//!   pending; measures steady-state insertion cost.
//! * `poll_front_one_ready` — `poll_front` on a deque whose single
//!   front future is immediately ready; minimum poll cost.
//! * `poll_front_pending_100` — `poll_front` on a deque of 100
//!   pending futures; measures the cost of scanning the deque.
//!
//! The same four scenarios are repeated for [`FutureDeque`] so the
//! atomic-coordination overhead of the sync variant can be compared
//! side by side with [`LocalFutureDeque`].
//!
//! [`FutureDeque`]: future_deque::FutureDeque
//! [`LocalFutureDeque`]: future_deque::LocalFutureDeque

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    allow(
        clippy::absolute_paths,
        clippy::allow_attributes_without_reason,
        clippy::exhaustive_structs,
        clippy::partial_pub_fields,
        clippy::pub_underscore_fields,
        clippy::cognitive_complexity,
        clippy::unnecessary_wraps,
        clippy::ignore_without_reason,
        clippy::default_trait_access,
        clippy::needless_pass_by_value,
        clippy::missing_assert_message,
        clippy::elidable_lifetime_names,
        clippy::needless_pass_by_ref_mut,
        clippy::doc_markdown,
        clippy::needless_for_each,
        clippy::redundant_clone,
        clippy::missing_docs_in_private_items,
        clippy::exit,
        clippy::undocumented_unsafe_blocks,
        clippy::multiple_unsafe_ops_per_block,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        unused_imports,
        unused_qualifications,
        dead_code,
        unreachable_pub,
        missing_debug_implementations,
        unnameable_types,
        non_local_definitions,
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
extern crate gungraun;

#[cfg(target_os = "linux")]
use std::future::Future;
#[cfg(target_os = "linux")]
use std::hint::black_box;
#[cfg(target_os = "linux")]
use std::pin::Pin;
#[cfg(target_os = "linux")]
use std::task::{Context, Poll, Waker};

#[cfg(target_os = "linux")]
use future_deque::{FutureDeque, LocalFutureDeque};
#[cfg(target_os = "linux")]
use gungraun::prelude::*;

#[cfg(target_os = "linux")]
const POPULATED_COUNT: usize = 100;

#[cfg(target_os = "linux")]
const PENDING_REMAINING: usize = 100;

// A future that returns `Pending` for `remaining` polls, then `Ready(value)`.
// Matches the helper used in the Criterion bench so wall-clock and cycle
// measurements exercise the same inner future shape.
#[cfg(target_os = "linux")]
struct CountdownFuture {
    remaining: usize,
    value: u64,
}

#[cfg(target_os = "linux")]
impl Unpin for CountdownFuture {}

#[cfg(target_os = "linux")]
impl CountdownFuture {
    fn new(remaining: usize, value: u64) -> Self {
        Self { remaining, value }
    }
}

#[cfg(target_os = "linux")]
impl Future for CountdownFuture {
    type Output = u64;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u64> {
        let this = self.get_mut();
        if this.remaining == 0 {
            Poll::Ready(this.value)
        } else {
            this.remaining = this.remaining.wrapping_sub(1);
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }
}

// ---------- LocalFutureDeque setup helpers ----------

#[cfg(target_os = "linux")]
fn empty_local() -> LocalFutureDeque<u64> {
    LocalFutureDeque::new()
}

#[cfg(target_os = "linux")]
fn populated_local_pending() -> LocalFutureDeque<u64> {
    let mut deque = LocalFutureDeque::new();
    for i in 0..POPULATED_COUNT {
        deque.push_back(CountdownFuture::new(PENDING_REMAINING, i as u64));
    }
    deque
}

#[cfg(target_os = "linux")]
fn single_ready_local() -> LocalFutureDeque<u64> {
    let mut deque = LocalFutureDeque::new();
    deque.push_back(CountdownFuture::new(0, 0));
    deque
}

// ---------- FutureDeque setup helpers ----------

#[cfg(target_os = "linux")]
fn empty_sync() -> FutureDeque<u64> {
    FutureDeque::new()
}

#[cfg(target_os = "linux")]
fn populated_sync_pending() -> FutureDeque<u64> {
    let mut deque = FutureDeque::new();
    for i in 0..POPULATED_COUNT {
        deque.push_back(CountdownFuture::new(PENDING_REMAINING, i as u64));
    }
    deque
}

#[cfg(target_os = "linux")]
fn single_ready_sync() -> FutureDeque<u64> {
    let mut deque = FutureDeque::new();
    deque.push_back(CountdownFuture::new(0, 0));
    deque
}

#[cfg(target_os = "linux")]
fn single_ready_local_with_waker() -> (LocalFutureDeque<u64>, &'static Waker) {
    (single_ready_local(), Waker::noop())
}

#[cfg(target_os = "linux")]
fn populated_local_pending_with_waker() -> (LocalFutureDeque<u64>, &'static Waker) {
    (populated_local_pending(), Waker::noop())
}

#[cfg(target_os = "linux")]
fn single_ready_sync_with_waker() -> (FutureDeque<u64>, &'static Waker) {
    (single_ready_sync(), Waker::noop())
}

#[cfg(target_os = "linux")]
fn populated_sync_pending_with_waker() -> (FutureDeque<u64>, &'static Waker) {
    (populated_sync_pending(), Waker::noop())
}

// ---------- LocalFutureDeque benches ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::empty(empty_local())]
fn push_back_empty(mut deque: LocalFutureDeque<u64>) -> LocalFutureDeque<u64> {
    deque.push_back(CountdownFuture::new(0, 42));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(populated_local_pending())]
fn push_back_into_100(mut deque: LocalFutureDeque<u64>) -> LocalFutureDeque<u64> {
    deque.push_back(CountdownFuture::new(0, 42));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::one_ready(single_ready_local_with_waker())]
fn poll_front_one_ready(input: (LocalFutureDeque<u64>, &'static Waker)) -> LocalFutureDeque<u64> {
    let (mut deque, waker) = input;
    let cx = Context::from_waker(waker);
    _ = black_box(deque.poll_front(&cx));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::pending_100(populated_local_pending_with_waker())]
fn poll_front_pending_100(input: (LocalFutureDeque<u64>, &'static Waker)) -> LocalFutureDeque<u64> {
    let (mut deque, waker) = input;
    let cx = Context::from_waker(waker);
    _ = black_box(deque.poll_front(&cx));
    deque
}

// ---------- FutureDeque (sync) benches ----------

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::empty(empty_sync())]
fn sync_push_back_empty(mut deque: FutureDeque<u64>) -> FutureDeque<u64> {
    deque.push_back(CountdownFuture::new(0, 42));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::populated(populated_sync_pending())]
fn sync_push_back_into_100(mut deque: FutureDeque<u64>) -> FutureDeque<u64> {
    deque.push_back(CountdownFuture::new(0, 42));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::one_ready(single_ready_sync_with_waker())]
fn sync_poll_front_one_ready(input: (FutureDeque<u64>, &'static Waker)) -> FutureDeque<u64> {
    let (mut deque, waker) = input;
    let cx = Context::from_waker(waker);
    _ = black_box(deque.poll_front(&cx));
    deque
}

#[cfg(target_os = "linux")]
#[library_benchmark]
#[bench::pending_100(populated_sync_pending_with_waker())]
fn sync_poll_front_pending_100(input: (FutureDeque<u64>, &'static Waker)) -> FutureDeque<u64> {
    let (mut deque, waker) = input;
    let cx = Context::from_waker(waker);
    _ = black_box(deque.poll_front(&cx));
    deque
}

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = local_group,
    benchmarks = [
        push_back_empty,
        push_back_into_100,
        poll_front_one_ready,
        poll_front_pending_100
    ]
);

#[cfg(target_os = "linux")]
library_benchmark_group!(
    name = sync_group,
    benchmarks = [
        sync_push_back_empty,
        sync_push_back_into_100,
        sync_poll_front_one_ready,
        sync_poll_front_pending_100
    ]
);

#[cfg(target_os = "linux")]
main!(library_benchmark_groups = local_group, sync_group);
