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

Mutation testing excludes the real platform's clock-forwarding methods:
distinguishing their constant replacements would require real-time assertions.
Deterministic tests above that boundary cover clock selection and measurement
arithmetic. Introducing another mock layer inside the forwarders would test
that layer rather than the operating-system boundary.

## Metrics and reports

Each operation folds complete spans into running totals and a shared statistics
accumulator. The accumulator owns the through-origin fit, weighted by squared
iteration counts, and its confidence interval. Span duration is not divided by
the iteration count before accumulation.

Reports clone accumulated metrics instead of retaining live measurement state.
Merging combines the accumulators as well as totals; report accessors expose the
finite fitted slope as a duration. Library fixtures cover this conversion
independently of clocks, including absent, undefined, zero and nonzero rates.
Unequal batch sizes and rates distinguish the fitted result from arithmetic
averages.

## Output boundary

The session owns drop-time eligibility and independent destination selection.
It builds one detached report after releasing measurement locks, then passes it
to the platform's output adapters. Real and fake platforms share construction
defaults and lifecycle logic; the fake retains reports by destination so unit
tests observe actual session destruction without stdout or filesystem access.

JSON preparation produces file names and serialized contents in memory,
rejecting sanitized-name collisions before the filesystem adapter runs. Unit
tests cover preparation, statistics and empty-output decisions; integration
tests cover real directories, overwrite and failure behavior. An isolated child
process captures session stdout and files without changing the parent's environment.

Mutation exclusions cover only platform forwarding and external-output adapters.
The Cargo target resolver and filesystem writes require integration tests, while
session eligibility, destination choices and JSON preparation remain in the
library mutation harness. Report wording is not used to distinguish lifecycle
mutations: captured reports expose metrics and real-output assertions identify
the caller's operation.
