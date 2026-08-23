# cbh_cli implementation

`cbh_cli` supports the command surface specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the `clap` parsing boundary, help organization, and translation from arguments into
the values owned by `cbh_command`. It also classifies parser exits for the process entry point.
Command execution and application policy remain outside this crate, keeping parser dependencies
and parser-specific concerns out of command implementations.
