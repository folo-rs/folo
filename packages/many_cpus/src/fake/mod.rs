//! Fake hardware implementation for testing.
//!
//! This module simulates hardware configurations for testing purposes. Fake hardware allows
//! tests to verify behavior under various hardware scenarios without requiring actual hardware.
//!
//! Only available when the `test-util` feature is enabled.
//!
//! # Basic usage
//!
//! ```
//! use many_cpus::SystemHardware;
//! use many_cpus::fake::HardwareBuilder;
//! use new_zealand::nz;
//!
//! let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(4), nz!(2)));
//!
//! assert_eq!(hardware.max_processor_count(), 4);
//! assert_eq!(hardware.max_memory_region_count(), 2);
//! ```
//!
//! # Designing testable code
//!
//! To make your code testable with fake hardware, accept [`crate::SystemHardware`] as a parameter
//! instead of always calling [`crate::SystemHardware::current()`]. This allows tests to substitute
//! fake hardware while production code uses real hardware.
//!
//! ```
//! use std::num::NonZero;
//!
//! use many_cpus::{ProcessorSet, SystemHardware};
//!
//! fn spawn_workers(hardware: &SystemHardware, count: NonZero<usize>) -> Option<ProcessorSet> {
//!     hardware.processors().take(count)
//! }
//! ```
//!
//! # Example: unit testing with fake hardware
//!
//! ```
//! use std::num::NonZero;
//!
//! use many_cpus::fake::HardwareBuilder;
//! use many_cpus::{ProcessorSet, SystemHardware};
//! use new_zealand::nz;
//!
//! fn spawn_workers(hardware: &SystemHardware, count: NonZero<usize>) -> Option<ProcessorSet> {
//!     hardware.processors().take(count)
//! }
//!
//! // Test with enough processors.
//! let hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(8), nz!(1)));
//! let workers = spawn_workers(&hardware, nz!(4));
//! assert!(workers.is_some());
//! assert_eq!(workers.unwrap().len(), 4);
//!
//! // Test with insufficient processors.
//! let small_hardware = SystemHardware::fake(HardwareBuilder::from_counts(nz!(2), nz!(1)));
//! let workers = spawn_workers(&small_hardware, nz!(4));
//! assert!(workers.is_none());
//! ```
//!
//! # Custom processor configurations
//!
//! For fine-grained control over processor properties, use [`ProcessorBuilder`]:
//!
//! ```
//! use many_cpus::fake::{HardwareBuilder, ProcessorBuilder};
//! use many_cpus::{EfficiencyClass, SystemHardware};
//!
//! let hardware = SystemHardware::fake(
//!     HardwareBuilder::new()
//!         .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Performance))
//!         .processor(ProcessorBuilder::new().efficiency_class(EfficiencyClass::Efficiency)),
//! );
//!
//! assert_eq!(hardware.processors().len(), 2);
//! ```
//!
//! # Isolation
//!
//! Each fake hardware instance is independent, ensuring that multiple fake instances can
//! coexist in parallel tests without interference.

mod builder;
mod processor_builder;

pub(crate) mod platform;

pub use builder::HardwareBuilder;
pub(crate) use platform::FakePlatform;
pub use processor_builder::ProcessorBuilder;
