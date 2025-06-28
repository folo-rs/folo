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
    use std::sync::Arc;
    use std::thread;

    use futures::task::noop_waker_ref;
    use testing::with_watchdog;

    use super::*;

    #[test]
    fn local_event_future_set_then_await() {
        with_watchdog(|| {
            let event = LocalEvent::new();
            let (sender, _receiver) = event.by_ref();
            sender.send(42);

            let mut future = LocalEventFuture::new(&event);
            let mut context = Context::from_waker(noop_waker_ref());

            // Since the value is already set, polling should return Ready
            let result = Pin::new(&mut future).poll(&mut context);
            match result {
                Poll::Ready(value) => assert_eq!(value, 42),
                Poll::Pending => panic!("Expected Ready, got Pending"),
            }
        });
    }

    #[test]
    fn local_event_future_await_then_set() {
        with_watchdog(|| {
            let event = LocalEvent::new();
            let (sender, _receiver) = event.by_ref();

            let mut future = LocalEventFuture::new(&event);
            let mut context = Context::from_waker(noop_waker_ref());

            // First poll should return Pending since no value is set
            let result1 = Pin::new(&mut future).poll(&mut context);
            assert!(
                matches!(result1, Poll::Pending),
                "First poll should return Pending"
            );

            // Now send the value
            sender.send(42);

            // Second poll should return Ready with the value
            let result2 = Pin::new(&mut future).poll(&mut context);
            match result2 {
                Poll::Ready(value) => assert_eq!(value, 42),
                Poll::Pending => panic!("Expected Ready after value was sent, got Pending"),
            }
        });
    }

    #[test]
    fn event_future_set_then_await() {
        with_watchdog(|| {
            let event = Event::new();
            let (sender, _receiver) = event.by_ref();
            sender.send(42);

            let mut future = EventFuture::new(&event);
            let mut context = Context::from_waker(noop_waker_ref());

            // Since the value is already set, polling should return Ready
            let result = Pin::new(&mut future).poll(&mut context);
            match result {
                Poll::Ready(value) => assert_eq!(value, 42),
                Poll::Pending => panic!("Expected Ready, got Pending"),
            }
        });
    }

    #[test]
    fn event_future_await_then_set() {
        with_watchdog(|| {
            let event = Arc::new(Event::new());
            let event_clone = Arc::clone(&event);

            let mut future = EventFuture::new(&*event);
            let mut context = Context::from_waker(noop_waker_ref());

            // First poll should return Pending since no value is set
            let result1 = Pin::new(&mut future).poll(&mut context);
            assert!(
                matches!(result1, Poll::Pending),
                "First poll should return Pending"
            );

            // Set the value from another thread context
            thread::spawn(move || {
                let (sender, _receiver) = event_clone.by_ref();
                sender.send(42);
            })
            .join()
            .unwrap();

            // Second poll should return Ready with the value
            let result2 = Pin::new(&mut future).poll(&mut context);
            match result2 {
                Poll::Ready(value) => assert_eq!(value, 42),
                Poll::Pending => panic!("Expected Ready after value was sent, got Pending"),
            }
        });
    }
}
