//! Ready-to-use policies for common processor-locality choices.
//!
//! Each type configures a fixed worker count but differs in how submissions share those workers:
//! one count per submitting processor, one count per memory region, or one count for the entire
//! system. These types follow the same [`WorkerPolicy`][crate::worker_policy::WorkerPolicy] contract
//! as custom policies and can be passed directly to
//! [`PoolBuilder::worker_policy`][crate::PoolBuilder::worker_policy].

mod workers_per_memory_region;
mod workers_per_processor;
mod workers_per_system;

pub use workers_per_memory_region::WorkersPerMemoryRegion;
pub use workers_per_processor::WorkersPerProcessor;
pub use workers_per_system::WorkersPerSystem;
