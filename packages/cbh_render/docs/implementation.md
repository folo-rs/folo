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

The renderer owns the shared coverage and analysis-outcome projections. Coverage derives
the judged-series account and no-findings headline from the detector census; finding presence
and that coverage select the analysis outcome. Text, Markdown, JSON and the shell's typed
or file outcome use these projections rather than maintaining independent verdict rules.
Collection-platform completeness belongs to workflow evidence, outside analyzer-series coverage.

## Rendering benchmarks

`cbh_render_reports` measures the production in-memory rendering functions, not detection or
output persistence. Its `private-test-util` fixtures preconstruct globally ranked history
findings, partition summaries and report metadata. Text, full Markdown and JSON share low/high
finding counts across fixed machine partitions and short before/after chart histories, so the
size axis measures finding formatting and chart generation rather than history growth.
The text-only no-findings case covers the quiet history report's early return.

Markdown summary cases use the production default cap: the low case retains every finding,
while the high case truncates the ranked list. Untimed assertions verify complete full-report
identities, partition membership and chart presence, and exact summary retention/omission.
Fixture assembly and these assertions are outside every measured invocation.

In-process library tests verify fixture partitioning, judged-series counts, ranked changes
and the exact chart-regime boundary independently of rendered output. They exercise the
untimed assertions on valid reports and deliberately incomplete workloads, so library-only
mutation testing covers the benchmark support without running benchmark targets.

`cbh_render_reports_cg` pairs the full-report and quiet-text scenarios with identical fixtures.
It counts formatting, charting, serialization and allocator instructions, not operating-system
allocation latency. The summary intentionally has no Callgrind counterpart: its production
`HashSet` uses randomized hashing, so probe counts can vary independently of source changes.
Criterion still measures that real production path without changing its hasher for benchmarks.
