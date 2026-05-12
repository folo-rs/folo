//! Future types returned by event `wait()` methods.
//!
//! These futures must be pinned before polling.

pub use crate::auto::{AutoResetWaitFuture, EmbeddedAutoResetWaitFuture};
pub use crate::local_auto::{EmbeddedLocalAutoResetWaitFuture, LocalAutoResetWaitFuture};
pub use crate::local_manual::{EmbeddedLocalManualResetWaitFuture, LocalManualResetWaitFuture};
pub use crate::manual::{EmbeddedManualResetWaitFuture, ManualResetWaitFuture};
