//! Native integration-test resources, not part of the product.
//!
//! A security identifier (SID) names a Windows account. A discretionary access
//! control list (DACL) specifies permissions; Security Descriptor Definition
//! Language (SDDL) represents those permissions as text.

mod harness;
pub mod native;
pub mod store;

pub use harness::*;
