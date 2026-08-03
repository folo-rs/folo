# cbh_command implementation

`cbh_command` supports the command surface specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the dependency-light command and option representation shared by the parser and
command implementations. It contains command inputs but owns neither argument parsing nor command
execution. This boundary lets those layers agree on one vocabulary without coupling execution to
`clap` or coupling parsing to storage and analysis dependencies.
