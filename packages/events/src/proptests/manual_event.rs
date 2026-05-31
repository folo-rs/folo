//! Abstraction over the two manual-reset event variants exercised by the
//! property tests.
//!
//! The trait's contract is the union of inherent methods both
//! [`LocalManualResetEvent`] and [`ManualResetEvent`] already expose;
//! implementations are one-line forwarders. Kept private to the proptest
//! subtree because it exists solely to deduplicate the test harness.

use std::future::Future;

use crate::local_manual::LocalManualResetWaitFuture;
use crate::manual::ManualResetWaitFuture;
use crate::{LocalManualResetEvent, ManualResetEvent};

pub(super) trait ManualEvent: 'static {
    type WaitFuture: Future<Output = ()>;

    fn create() -> Self;
    fn set(&self);
    fn reset(&self);
    fn try_wait(&self) -> bool;
    fn wait(&self) -> Self::WaitFuture;
}

impl ManualEvent for LocalManualResetEvent {
    type WaitFuture = LocalManualResetWaitFuture;

    fn create() -> Self {
        Self::boxed()
    }

    fn set(&self) {
        Self::set(self);
    }

    fn reset(&self) {
        Self::reset(self);
    }

    fn try_wait(&self) -> bool {
        Self::try_wait(self)
    }

    fn wait(&self) -> Self::WaitFuture {
        Self::wait(self)
    }
}

impl ManualEvent for ManualResetEvent {
    type WaitFuture = ManualResetWaitFuture;

    fn create() -> Self {
        Self::boxed()
    }

    fn set(&self) {
        Self::set(self);
    }

    fn reset(&self) {
        Self::reset(self);
    }

    fn try_wait(&self) -> bool {
        Self::try_wait(self)
    }

    fn wait(&self) -> Self::WaitFuture {
        Self::wait(self)
    }
}
