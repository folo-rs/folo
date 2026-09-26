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

Mutation testing targets the production exporter, delta state and streaming iterator, not
the trivial histogram-state forwarder used by allocation tests and benchmarks. The metric
lookup assertion helper is also excluded: it consumes populated SDK snapshots that have no
public in-memory constructors and are obtained through the SDK collection pipeline. Its
consumers remain integration tests rather than running real SDK collection solely to test
an assertion helper in the library harness.

Test-reader construction and pipeline registration are also integration-only mutation
exclusions: provider construction runs SDK resource detection, and a live pipeline has no
public constructor independent of that provider. The integration tests assert metrics from
the returned provider through its paired reader, covering both connections. The reader's
collection helper remains a mutation target. Its unit test verifies that an unregistered
reader fails rather than returning an empty snapshot, without initializing resource detectors
or reading a clock. Successful collection remains covered by the SDK integration assertions.
These exclusions are function-local and do not exclude the production algorithms or the
rest of the test-reader support.

### Collection boundary

The single-iteration adapter connects `Report::collect()` to the exporter. Its real
collection-to-export behavior belongs to integration tests: recording real `nm` events
initializes platform clocks, and the SDK provider performs OS resource detection. The
publisher uses a frozen clock and explicit iteration calls in these tests, with no waits
or elapsed-time assertions. Separate test binaries isolate the process-wide event registry.
They verify initial publication, unchanged observations and fresh observations across
collections, including retention of counter delta state.

Private builder, registry and delta-state assertions use OpenTelemetry's no-op provider.
They exercise the same exporter without initializing the SDK's resource detectors or clock.
Assertions about exported SDK metric data use the existing supplied-report driver in the
integration target. These reports do not register events, so they can share a binary with
real collection coverage without contaminating the process-wide event registry.

Mutation exclusions cover only that adapter and its trivial integration-test forwarder.
The supplied-report driver, exporter and delta algorithms remain mutation targets.
Pre-built reports exercise export behavior but do not establish that real collection
feeds it. Injecting a report supplier would test substitute wiring rather than this
connection, so the publisher does not acquire a collection abstraction solely for mutation
testing. The perpetual publishing loop has a separate no-hang exclusion because its mutants
can stop yielding while mutation testing disables watchdogs.

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
