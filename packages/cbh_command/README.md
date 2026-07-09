# cbh_command

Implementation crate for [`cargo-bench-history`](https://github.com/folo-rs/folo). Do
not depend on this directly — it carries the parsed command model (the `Command` enum and
the per-subcommand option value types) that the CLI parser produces and the commands
consume. It has no stable public API; use the `cargo-bench-history` tool instead.
