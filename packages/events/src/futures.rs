//! Future implementations for event receivers.
//!
//! This module provides `Future` implementations that wrap event types
//! to provide async/await support.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use super::once::OnceEvent;
use crate::Disconnected;

/// A `Future` that resolves when a thread-safe event receives a value.
#[derive(Debug)]
pub(crate) struct EventFuture<'a, T>
where
    T: Send,
{
    event: &'a OnceEvent<T>,
}

impl<'a, T> EventFuture<'a, T>
where
    T: Send,
{
    /// Creates a new future for a thread-safe event.
    pub(crate) fn new(event: &'a OnceEvent<T>) -> Self {
        Self { event }
    }
}

impl<T> Future for EventFuture<'_, T>
where
    T: Send,
{
    type Output = Result<T, Disconnected>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.event
            .poll(cx.waker())
            .map_or_else(|| Poll::Pending, Poll::Ready)
    }
}
