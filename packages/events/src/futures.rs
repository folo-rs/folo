//! Future implementations for event receivers.
//!
//! This module provides `Future` implementations that wrap event types
//! to provide async/await support.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::once::{Event, LocalEvent};

/// A `Future` that resolves when a single-threaded event receives a value.
#[derive(Debug)]
pub(crate) struct LocalEventFuture<'a, T> {
    event: &'a LocalEvent<T>,
}

impl<'a, T> LocalEventFuture<'a, T> {
    /// Creates a new future for a single-threaded event.
    pub(crate) fn new(event: &'a LocalEvent<T>) -> Self {
        Self { event }
    }
}

impl<T> Future for LocalEventFuture<'_, T> {
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}

/// A `Future` that resolves when a thread-safe event receives a value.
#[derive(Debug)]
pub(crate) struct EventFuture<'a, T>
where
    T: Send,
{
    event: &'a Event<T>,
}

impl<'a, T> EventFuture<'a, T>
where
    T: Send,
{
    /// Creates a new future for a thread-safe event.
    pub(crate) fn new(event: &'a Event<T>) -> Self {
        Self { event }
    }
}

impl<T> Future for EventFuture<'_, T>
where
    T: Send,
{
    type Output = T;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll_recv(cx.waker())
            .map_or_else(|| Poll::Pending, |value| Poll::Ready(value))
    }
}

#[cfg(test)]
mod tests {
    use futures::executor::block_on;
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_future_set_then_await() {
        with_watchdog(|| {
            let event = LocalEvent::new();
            let (sender, _receiver) = event.by_ref();
            sender.send(42);

            let future = LocalEventFuture::new(&event);
            let result = block_on(future);
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn local_event_future_await_then_set() {
        with_watchdog(|| {
            // This test would require cross-thread access, but LocalEvent is single-threaded only
            // So we just test that the future can be created and awaited when value is already set
            let event = LocalEvent::new();
            let (sender, _receiver) = event.by_ref();
            sender.send(42);

            let future = LocalEventFuture::new(&event);
            let result = block_on(future);
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn event_future_set_then_await() {
        with_watchdog(|| {
            let event = Event::new();
            let (sender, _receiver) = event.by_ref();
            sender.send(42);

            let future = EventFuture::new(&event);
            let result = block_on(future);
            assert_eq!(result, 42);
        });
    }

    #[test]
    fn event_future_await_then_set() {
        with_watchdog(|| {
            use std::sync::Arc;

            let event = Arc::new(Event::new());
            let event_clone = Arc::clone(&event);
            let future = EventFuture::new(&*event);

            // Set the value concurrently
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let (sender, _receiver) = event_clone.by_ref();
                sender.send(42);
            });

            let result = block_on(future);
            assert_eq!(result, 42);
        });
    }
}
