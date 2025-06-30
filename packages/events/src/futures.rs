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
    #[allow(
        dead_code,
        reason = "Utility function kept for potential future use and API completeness"
    )]
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
