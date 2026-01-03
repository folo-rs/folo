// Platform abstraction layer for cargo-detect-package.
//
// This module provides abstractions over filesystem operations to enable mocking in tests.
// The pattern follows the three-layer approach: abstraction (trait) → facade (enum) → real
// implementation, mirroring the PAL structure used in `many_cpus`.

mod filesystem;

pub(crate) use filesystem::*;
