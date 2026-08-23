# nm design

`nm` collects event metrics with low observation overhead in highly concurrent
applications. The API favors integer measurements and thread-local event handles so
instrumentation can remain practical on frequently executed paths.

## Events and observations

An event has a name and an optional histogram configuration. Each observation contributes
to its occurrence count and magnitude statistics. Occurrences without a meaningful
magnitude use `observe_once`, while duration-specific operations convert whole milliseconds
into magnitudes. Batch observation records repeated occurrences with a common magnitude in
one operation.

Event names use the `big_medium_small_units` convention. A runtime discriminator belongs in
the name together with the property and unit it describes, so reports remain interpretable
without external naming context.

Event handles are single-threaded. A multithreaded application creates an identically
configured handle on each observing thread; reports combine those observations by event
name. Event names may be constructed at runtime, but each unique name can be registered
only once per thread. Registering the same name again on that thread panics.

## Histograms

A histogram supplements the count, sum, and mean with a magnitude distribution. Callers
choose ascending inclusive bucket boundaries that distinguish ranges meaningful to their
workload. An implicit final range captures magnitudes above the configured boundaries.

## Duration observation

Duration observation uses a low-precision clock to keep frequently executed instrumentation
inexpensive. Its granularity is suitable for millisecond-scale operations. Faster operations
are measured in batches so the measured work dominates observation overhead.

## Publishing and reports

Pull publishing is the default: reports read the latest metrics without an explicit
publication step. Push publishing can reduce observation overhead, but the observing thread
must publish through its metrics pusher before those metrics appear in reports. The
publishing model is selected independently for each event.

A report combines published metrics for matching event names across threads. Reports are
cumulative from process start; collection does not reset event metrics. Human-readable
rendering and per-event inspection expose the same collected data for terminal and exporter
use cases.

## Numeric and panic policies

Observation does not panic for mathematical overflow or underflow. Instantaneous or
cumulative values near integer limits may yield unspecified metric values, so callers keep
expected workloads away from those limits.

Registration may panic for invalid event configuration. Report collection panics when
matching event names from different threads have incompatible configurations.

The [implementation guide](implementation.md) describes how the `nm` package family
realizes these behaviors.
