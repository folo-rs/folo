//! Built-in policies for routing work to fixed worker counts.

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
mod workers_per_memory_region;
mod workers_per_processor;
mod workers_per_system;

pub use workers_per_memory_region::WorkersPerMemoryRegion;
pub use workers_per_processor::WorkersPerProcessor;
pub use workers_per_system::WorkersPerSystem;
