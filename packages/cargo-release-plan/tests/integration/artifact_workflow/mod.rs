//! Complete artifact workflows over independently mutable Git and Cargo workspaces.
//!
//! Libtest's deterministic name order starts these long subprocess chains alongside
//! the short classification cases instead of leaving them at the suite's tail.

mod captured;
mod evidence;
mod preview;
mod preview_safety;
