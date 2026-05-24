//! Callgrind benchmarks for the `future_deque` crate.
//!
//! Paired with `future_deque.rs` (the Criterion benches for the same
//! types) which exercises the build-and-drain shape with mixed
//! activity ratios. The Callgrind benches isolate the per-call cost of
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
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Valgrind is Linux-only.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::future::Future;
    use std::hint::black_box;
    use std::pin::Pin;
    use std::task::{Context, Poll, Waker};

    use future_deque::{FutureDeque, LocalFutureDeque};
    use gungraun::prelude::*;

    const POPULATED_COUNT: usize = 100;

    const PENDING_REMAINING: usize = 100;

    // A future that returns `Pending` for `remaining` polls, then `Ready(value)`.
    // Matches the helper used in the Criterion bench so wall-clock and Callgrind
    // measurements exercise the same inner future shape.
    struct CountdownFuture {
        remaining: usize,
        value: u64,
    }

    impl Unpin for CountdownFuture {}

    impl CountdownFuture {
        fn new(remaining: usize, value: u64) -> Self {
            Self { remaining, value }
        }
    }

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

    fn empty_local() -> LocalFutureDeque<u64> {
        LocalFutureDeque::new()
    }

    fn populated_local_pending() -> LocalFutureDeque<u64> {
        let mut deque = LocalFutureDeque::new();
        for i in 0..POPULATED_COUNT {
            deque.push_back(CountdownFuture::new(PENDING_REMAINING, i as u64));
        }
        deque
    }

    fn single_ready_local() -> LocalFutureDeque<u64> {
        let mut deque = LocalFutureDeque::new();
        deque.push_back(CountdownFuture::new(0, 0));
        deque
    }

    // ---------- FutureDeque setup helpers ----------

    fn empty_sync() -> FutureDeque<u64> {
        FutureDeque::new()
    }

    fn populated_sync_pending() -> FutureDeque<u64> {
        let mut deque = FutureDeque::new();
        for i in 0..POPULATED_COUNT {
            deque.push_back(CountdownFuture::new(PENDING_REMAINING, i as u64));
        }
        deque
    }

    fn single_ready_sync() -> FutureDeque<u64> {
        let mut deque = FutureDeque::new();
        deque.push_back(CountdownFuture::new(0, 0));
        deque
    }

    fn single_ready_local_with_waker() -> (LocalFutureDeque<u64>, &'static Waker) {
        (single_ready_local(), Waker::noop())
    }

    fn populated_local_pending_with_waker() -> (LocalFutureDeque<u64>, &'static Waker) {
        (populated_local_pending(), Waker::noop())
    }

    fn single_ready_sync_with_waker() -> (FutureDeque<u64>, &'static Waker) {
        (single_ready_sync(), Waker::noop())
    }

    fn populated_sync_pending_with_waker() -> (FutureDeque<u64>, &'static Waker) {
        (populated_sync_pending(), Waker::noop())
    }

    // ---------- LocalFutureDeque benches ----------

    #[library_benchmark]
    #[bench::empty(empty_local())]
    fn push_back_empty(mut deque: LocalFutureDeque<u64>) -> LocalFutureDeque<u64> {
        deque.push_back(CountdownFuture::new(0, 42));
        deque
    }

    #[library_benchmark]
    #[bench::populated(populated_local_pending())]
    fn push_back_into_100(mut deque: LocalFutureDeque<u64>) -> LocalFutureDeque<u64> {
        deque.push_back(CountdownFuture::new(0, 42));
        deque
    }

    #[library_benchmark]
    #[bench::one_ready(single_ready_local_with_waker())]
    fn poll_front_one_ready(
        input: (LocalFutureDeque<u64>, &'static Waker),
    ) -> LocalFutureDeque<u64> {
        let (mut deque, waker) = input;
        let cx = Context::from_waker(waker);
        _ = black_box(deque.poll_front(&cx));
        deque
    }

    #[library_benchmark]
    #[bench::pending_100(populated_local_pending_with_waker())]
    fn poll_front_pending_100(
        input: (LocalFutureDeque<u64>, &'static Waker),
    ) -> LocalFutureDeque<u64> {
        let (mut deque, waker) = input;
        let cx = Context::from_waker(waker);
        _ = black_box(deque.poll_front(&cx));
        deque
    }

    // ---------- FutureDeque (sync) benches ----------

    #[library_benchmark]
    #[bench::empty(empty_sync())]
    fn sync_push_back_empty(mut deque: FutureDeque<u64>) -> FutureDeque<u64> {
        deque.push_back(CountdownFuture::new(0, 42));
        deque
    }

    #[library_benchmark]
    #[bench::populated(populated_sync_pending())]
    fn sync_push_back_into_100(mut deque: FutureDeque<u64>) -> FutureDeque<u64> {
        deque.push_back(CountdownFuture::new(0, 42));
        deque
    }

    #[library_benchmark]
    #[bench::one_ready(single_ready_sync_with_waker())]
    fn sync_poll_front_one_ready(input: (FutureDeque<u64>, &'static Waker)) -> FutureDeque<u64> {
        let (mut deque, waker) = input;
        let cx = Context::from_waker(waker);
        _ = black_box(deque.poll_front(&cx));
        deque
    }

    #[library_benchmark]
    #[bench::pending_100(populated_sync_pending_with_waker())]
    fn sync_poll_front_pending_100(input: (FutureDeque<u64>, &'static Waker)) -> FutureDeque<u64> {
        let (mut deque, waker) = input;
        let cx = Context::from_waker(waker);
        _ = black_box(deque.poll_front(&cx));
        deque
    }

    library_benchmark_group!(
        name = local_group,
        benchmarks = [
            push_back_empty,
            push_back_into_100,
            poll_front_one_ready,
            poll_front_pending_100
        ]
    );

    library_benchmark_group!(
        name = sync_group,
        benchmarks = [
            sync_push_back_empty,
            sync_push_back_into_100,
            sync_poll_front_one_ready,
            sync_poll_front_pending_100
        ]
    );
}

#[cfg(target_os = "linux")]
pub use linux::{local_group, sync_group};

#[cfg(target_os = "linux")]
gungraun::main!(library_benchmark_groups = local_group, sync_group);
