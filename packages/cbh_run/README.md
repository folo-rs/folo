# cbh_run

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do not
depend on this directly — it carries the result model a command run produces (the
`RunOutcome` it returns and the `RunError` it fails with) and the `OutputWriter` port that
writes the per-format reports. It has no stable public API; use the `cargo-bench-history`
tool instead.
