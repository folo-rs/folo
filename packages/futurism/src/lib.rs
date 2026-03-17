#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Utilities for working with futures.
//!
//! This crate provides collection types for managing groups of futures with precise
//! control over polling order and result retrieval.
//!
//! # Types
//!
//! * [`FutureDeque`] — a thread-mobile (`Send`) deque of futures. Requires inserted
//!   futures to be `Send`.
//! * [`LocalFutureDeque`] — a single-threaded (`!Send`) variant that accepts any future.
//!
//! Both types poll active futures in deterministic front-to-back order and allow results
//! to be popped from either end with strict deque semantics (only the actual front or back
//! item can be popped, and only if it has completed).
//!
//! # Basic usage
//!
//! ```rust
//! use std::task::{Context, Poll, Waker};
//!
//! use futurism::LocalFutureDeque;
//!
//! let mut deque = LocalFutureDeque::new();
//!
//! deque.push_back(async { 1 });
//! deque.push_back(async { 2 });
//! deque.push_front(async { 0 });
//!
//! let waker = Waker::noop();
//! let cx = &mut Context::from_waker(waker);
//!
//! assert_eq!(deque.poll_front(cx), Poll::Ready(Some(0)));
//! assert_eq!(deque.poll_front(cx), Poll::Ready(Some(1)));
//! assert_eq!(deque.poll_front(cx), Poll::Ready(Some(2)));
//! assert_eq!(deque.poll_front(cx), Poll::Ready(None));
//! ```

mod erased_future;
mod future_deque;
mod future_deque_core;
mod local_future_deque;
mod waker_meta;

pub use future_deque::*;
pub use local_future_deque::*;
