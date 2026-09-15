# Processor time tracking architecture

The [measurement design](design.md) separates clock sampling, span recording and
report statistics. The platform abstraction supplies cumulative thread and
process processor clocks. Spans retain their opening sample and share the
operation's metrics; closing a span adds a delta and iteration count.

## Clock boundary and arithmetic

The real platform forwards to `cpu_time`, which owns operating-system clock
access. A cloneable fake platform supplies independently controlled thread and
process samples to library unit tests. Tests use different opening samples and
deltas for the clocks so that choosing the wrong source is observable.

Closing samples are subtracted with saturation at zero. A span's nanosecond
delta saturates at the capacity of `u64` before accumulation into wider totals.
These are defensive arithmetic choices, not promises about clock resolution or
elapsed execution time. Unit fixtures exercise them without waiting for a clock.

Mutation testing excludes only the real platform's clock-forwarding methods:
distinguishing their constant replacements would require real-time assertions.
The facade and measurement arithmetic remain subject to deterministic mutation
coverage. Introducing another mock layer inside the forwarders would test that
layer rather than the operating-system boundary.

## Metrics and reports

Each operation folds complete spans into running totals and the shared
`folo_utils::SpanAccumulator`. The accumulator owns the through-origin fit,
weighted by squared iteration counts, and its confidence interval. Span duration
is not divided by the iteration count before accumulation.

Reports clone accumulated metrics instead of retaining live measurement state.
Merging combines the accumulators as well as totals; report accessors expose the
finite fitted slope as a duration. Library fixtures cover this conversion
independently of clocks, including absent, undefined, zero and nonzero rates.
Unequal batch sizes and rates distinguish the fitted result from arithmetic
averages.
