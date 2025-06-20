An object pool that guarantees pinning of its items and enables easy item access
via unsafe code by not maintaining any Rust references to its items.

More details in the [crate documentation](https://docs.rs/pinned_pool/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.