//! Example demonstrating backtrace tracking for debugging awaiting events.
//!
//! To run with backtraces enabled:
//!
//! ```bash
//! RUST_BACKTRACE=1 cargo run --example events_awaiter_backtrace
//! ```

use std::pin::pin;

use events::{ByRefEvent, OnceEvent, OnceReceiver};
use futures::executor::block_on;
use futures::task::{Context, noop_waker_ref};

fn main() {
    println!("=== Event Backtrace Debugging Example ===");
    println!("Run with RUST_BACKTRACE=1 to see backtraces");
    println!();

    block_on(async {
        let event = OnceEvent::<String>::new();
        let (_sender, receiver) = event.bind_by_ref();

        await_on_event_via_receiver(receiver);

        // This method is only available in debug builds, as it carries heavy performance overhead.
        #[cfg(debug_assertions)]
        event.inspect_awaiter(|backtrace| match backtrace {
            Some(backtrace) => println!("Awaiter backtrace: {backtrace}"),
            None => unreachable!("No awaiter found"),
        });
    });
}

// You will see the name of this function in the backtrace because it is doing the await.
fn await_on_event_via_receiver(receiver: OnceReceiver<String, ByRefEvent<'_, String>>) {
    let mut context = Context::from_waker(noop_waker_ref());
    let mut pinned = pin!(receiver);

    drop(pinned.as_mut().poll(&mut context));
}
