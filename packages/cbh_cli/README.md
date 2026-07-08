# cbh_cli

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do not
depend on this directly — it carries the `clap` argument-parsing surface that turns argv
into the typed command model, plus the `EarlyExit` handling for help and usage errors. It
has no stable public API; use the `cargo-bench-history` tool instead.
