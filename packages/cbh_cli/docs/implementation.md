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

Blessing mutations have a separate discriminant argument group: their help describes
unrestricted omitted axes, while query help describes host defaults. Both produce the
same raw option values; orchestration owns their distinct resolution policies.

Standalone Azure setup has its own argument group rather than flattening benchmark
environment options. Export admits partial deployment inputs; execution requires explicit
placement and repository inputs. Explicit custom access is a paired CLI constraint.
Current-user lookup conflicts with those explicit values and with offline export.
