//! Future types returned by `lock()` and `acquire()` methods.

pub use crate::local_mutex::{EmbeddedLocalMutexLockFuture, LocalMutexLockFuture};
pub use crate::local_semaphore::{
    EmbeddedLocalSemaphoreAcquireFuture, LocalSemaphoreAcquireFuture,
};
pub use crate::mutex::{EmbeddedMutexLockFuture, MutexLockFuture};
pub use crate::semaphore::{EmbeddedSemaphoreAcquireFuture, SemaphoreAcquireFuture};
