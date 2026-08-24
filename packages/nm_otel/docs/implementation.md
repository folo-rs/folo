# nm_otel implementation

## Package boundary

`nm_otel` is the documented public shell, while `nm_otel_impl` owns the implementation. The shell
explicitly re-exports only `Publisher` and `PublisherBuilder` so maintainer-facing items required
across crate boundaries cannot accidentally become public API. User-facing examples and the
behavioral contract therefore remain in this package rather than being duplicated by the private
implementation partition.

The two crates form one library and are versioned together. `nm_otel_impl` is not an independent
API or documentation owner; architectural changes in that partition are described here, and
user-visible changes are described in [the design document](design.md).

Production collection remains private to the publisher. The implementation crate's
`private-test-util` feature exposes a separately named one-iteration driver, pre-built report
driver, and histogram delta state only to in-workspace tests and benchmarks. The shell does not
forward this feature.

## Recording pipeline

On each publisher interval, the implementation obtains an aggregated report from `nm`, associates
each event with its OpenTelemetry instruments, and computes counter deltas from the report's count
and cumulative bucket values. It adds those deltas to counters and records the report's sum as a
gauge. Instrument state is retained between intervals so repeated publication does not recreate
instruments.

The histogram mapping records the aggregated values directly through counters and a gauge rather
than attempting to reconstruct the individual observations that produced them. This preserves
the information available in an `nm` report and keeps publication work proportional to the number
of events and buckets instead of the number of original observations.

Event instruments, bucket attributes, and previous cumulative values are cached when first seen.
The instrument names, including the underscore shift that keeps companion-shaped event names
apart, are derived at that point rather than per export. Once the event and histogram
configuration is established, the steady-state histogram path looks up this state, computes
deltas, and records them without allocating.

### Delta discontinuities and limits

OpenTelemetry counters accept only nonnegative increments. If an upstream cumulative count or
bucket total decreases, the publisher treats it as a discontinuity: it records no increment for
that collection and replaces the retained baseline. Later increases are measured from the
replacement baseline instead of being suppressed until they exceed the old value.

Converting non-cumulative histogram buckets into cumulative `u64` values clamps totals at
`u64::MAX`. The counter representation cannot encode a larger value, and wrapping would fabricate
a smaller total and a false discontinuity. Iterator indexing is not metric arithmetic and remains
checked as a structural invariant.

Both the delta state and the instrument cache are keyed by event name and hashed with the
standard library's `HashDoS`-resistant default rather than a faster non-cryptographic hasher.
Event names originate outside this library, so the hash has to hold up against a caller-chosen
name set rather than merely against accidental clustering; the resulting cost and per-process
variation in probe counts are accepted in the instruction-count benchmarks that cover the export
path.

The configured meter provider creates the instruments and is retained by the publisher so its
metric pipeline remains active for the publisher's lifetime. Its readers operate independently of
the publisher: the publisher interval determines when fresh `nm` values are recorded, while reader
and exporter configuration determines when OpenTelemetry collects and emits those values.
