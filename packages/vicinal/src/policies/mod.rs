//! Built-in policies for routing work to fixed worker counts.

mod workers_per_memory_region;
mod workers_per_processor;
mod workers_per_system;

pub use workers_per_memory_region::WorkersPerMemoryRegion;
pub use workers_per_processor::WorkersPerProcessor;
pub use workers_per_system::WorkersPerSystem;
