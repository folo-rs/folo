# cbh_probe

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do
not depend on this directly — it carries the environment probe (git and toolchain facts)
and the machine fingerprint that partitions benchmark results, and has no stable
public API. Use the `cargo-bench-history` tool instead.
