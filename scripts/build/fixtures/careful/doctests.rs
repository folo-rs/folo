//! Merged-doctest fixture for CarefulRunner.Tests.ps1.
//!
//! Each example must start with untouched library-global state.
//!
//! ```
//! use careful_doctests::observe;
//!
//! assert!(cfg!(careful));
//! observe();
//! ```
//!
//! A second example uses the same state, even when test threads are serialized.
//!
//! ```
//! use careful_doctests::observe;
//!
//! assert!(cfg!(careful));
//! observe();
//! ```

use std::env;
use std::sync::atomic::{AtomicBool, Ordering};

static OBSERVED: AtomicBool = AtomicBool::new(false);

/// Requires fresh process-global state and the runner's restored environment.
pub fn observe() {
    assert!(cfg!(careful));
    assert!(!OBSERVED.swap(true, Ordering::Relaxed));
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
        "FOLO_CAREFUL_BUILD_FLAGS",
    ] {
        assert!(env::var_os(name).is_none());
    }
}
