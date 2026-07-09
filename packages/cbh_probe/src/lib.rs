#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(docsrs, feature(doc_cfg))]
#![doc(hidden)]
#![expect(
    clippy::exhaustive_structs,
    reason = "this crate's `pub` items form an internal handoff boundary between the \
              cargo-bench-history sub-crates rather than a stable public API, so \
              exhaustive construction of its probe fact types by those in-workspace \
              consumers is intended"
)]

//! Implementation crate for [`cargo-bench-history`]; do not depend on this directly.
//!
//! The environment and hardware probe. The environment probe port discovers the git and
//! toolchain facts of a run (shelling out to `git` and `rustc`), and the machine
//! fingerprint derives a stable, reproducible key from the host's hardware so that
//! hardware-dependent results are only ever compared across equivalent machines. Split
//! out of the `cargo-bench-history` shell so this environment-sensing code is isolated
//! for mutation testing.
//!
//! Every item is re-exported flat from the crate root, so consumers write
//! `cbh_probe::SystemProbe` rather than reaching into a submodule.
//!
//! [`cargo-bench-history`]: https://github.com/folo-rs/folo

mod host;
mod machine;
mod probe;

pub use host::RustcInfo;
pub use machine::{HardwareProfile, resolve_machine_key};
pub use probe::{EnvironmentProbe, SystemProbe};
