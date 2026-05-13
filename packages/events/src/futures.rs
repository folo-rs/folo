//! Future types returned by event `wait()` methods.

pub use crate::auto::{AutoResetWaitFuture, EmbeddedAutoResetWaitFuture};
pub use crate::local_auto::{EmbeddedLocalAutoResetWaitFuture, LocalAutoResetWaitFuture};
pub use crate::local_manual::{EmbeddedLocalManualResetWaitFuture, LocalManualResetWaitFuture};
pub use crate::manual::{EmbeddedManualResetWaitFuture, ManualResetWaitFuture};
