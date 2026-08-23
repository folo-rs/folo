# `nm_otel_impl`

Internal implementation of the
[`nm_otel`](https://crates.io/crates/nm_otel) OpenTelemetry metrics publisher.
Applications and libraries depend on `nm_otel`, not this crate.

This crate exists so the `nm_otel` package can keep its published API surface minimal
while still permitting in-workspace tests and benchmarks (hosted here in `nm_otel_impl`)
to reach internal items that are not part of the `nm_otel` API. See the
[`nm_otel` implementation guide] for the package architecture.

Only the items re-exported by `nm_otel` are part of its public API contract.
Internal test and benchmark helpers are available only through the `private-test-util`
feature, which the `nm_otel` shell does not forward.

[`nm_otel` implementation guide]:
    https://github.com/folo-rs/folo/blob/main/packages/nm_otel/docs/implementation.md
