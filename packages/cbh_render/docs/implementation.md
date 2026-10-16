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
Full Markdown appends its complete coverage account after findings and report warnings, including
on the silent-result path. The same coverage projection supplies its totals and reason rows and
the JSON census. Every human report starts with a decision and coverage overview; missing
in-scope coverage remains explicit beside findings. Condensed summaries retain the overview
and full-analysis qualifications independently of their top-finding selection.

Quantity formatting reads `MetricKind::as_unit`. One scale per finding governs its comparison
values, signed deltas and plotted observations; JSON serialization bypasses display conversion.
Topology binning and branch comparison columns remain independent of unit scaling.
Time-series plots contain no prose, while retaining every topology column and gap.
Multiline diagnostics preserve their paragraph and bullet layout;
single-line identity escaping does not flatten those messages.

The analysis boundary supplies resolved inspection context separately from findings. The
renderer quotes complete argument values for each supported shell, fences embedded content
safely and refuses lossy or control-bearing command text. It does not read environment
variables, discover repository remotes or acquire data. Exact-current provenance qualifies
stored inspection rather than silently claiming the two sources are interchangeable.

Condensed output budgets whole vertical finding blocks, reserving space for the GitHub envelope.
Its range sketch includes actual zero in the shared linear span and normalizes finite quantities
before mapping them to a full-line grid. Nonnegative observations start at zero; finite negatives
extend to its left rather than being clipped. The zero label, bracketed base range and current
marker remain visible even when they share a display cell. An all-zero comparison has no span
to draw. Narrow readers scroll the code block rather than the page.
Rounding can collapse a visually tiny range to one cell. Per-finding numeric highlights stay
limited to current value/excess or before/after representatives/change.

Aggregate material qualifications cover every group before truncation. Detailed comparison
counts, inspection commands, exact machine identity and time-series evidence remain in full reports;
machine identity only enters a summary when needed to disambiguate otherwise matching findings.
The same coverage projection controls concise summary coverage and the complete full-report account.
Selection diagnostics have their own escaped-byte budget, separate from short identity labels.
Ordinary explanations retain every exclusion and remedy. Oversized diagnostics omit whole lines
with an explicit notice, continuing past long identity lines to retain later instructions.

Ghosts and configured ignores remain in the census total but sit outside the in-scope
denominator. Ignored-series disclosure is derived from that same census, including beside
findings and in truncated summaries. An all-excluded census retains `nothing_in_scope`;
there is no separate outcome or duplicated top-level exclusion counter.

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
allocation latency. Criterion additionally measures the compact summary blocks under the
production size budget.
