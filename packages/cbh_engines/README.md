# cbh_engines

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do
not depend on this directly — it carries the per-engine environment injection and output
parsers (Callgrind, Criterion, `alloc_tracker`, `all_the_time`) and the benchmark-output
harvesting port, and has no stable public API. Use the `cargo-bench-history` tool instead.
