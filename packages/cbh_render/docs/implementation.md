# cbh_render implementation

`cbh_render` implements the report behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns presentation of model facts and detected findings as text, Markdown, and JSON,
including report formatting and charts. It does not select stored data, detect findings, choose
output destinations, or write process streams. Presentation dependencies therefore remain
separate from the I/O-free detector and the application shell.
