#[cfg(debug_assertions)]
use std::cell::RefCell;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::sync::{Arc, Barrier};
use std::task::{self, Poll, Waker};
use std::{iter, thread};

use futures::executor::block_on;
use static_assertions::assert_impl_all;
#[cfg(debug_assertions)]
use testing::assert_panics_with;
use testing::with_watchdog;

use super::*;
use crate::Disconnected;
#[cfg(debug_assertions)]
use crate::assert_inspect_awaiters_is_reentrant;

// The payload satisfies only the bound that the pool's API requires (`Send`) and lacks every
// trait asserted here, so each of them is supplied by the pool's own synchronization and
// storage rather than inherited from the payload. A trait object payload also has to preserve
// the thread-safety traits (regression test for #142).
assert_impl_all!(RawEventPool<Box<dyn Send>>: Send, Sync, UnwindSafe, RefUnwindSafe);

mod concurrency;
mod diagnostics;
mod lifecycle;
