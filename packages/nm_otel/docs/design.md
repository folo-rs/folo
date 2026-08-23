# nm_otel design

`nm_otel` publishes metrics observed by `nm` through OpenTelemetry so applications can use
OpenTelemetry-compatible metric backends without changing their instrumentation.

## Publication model

Applications observe events through `nm`. The publisher periodically collects the accumulated
event data and records corresponding OpenTelemetry instruments through the configured meter
provider. Its interval controls this collection-and-recording cadence and defaults to one minute.

The meter provider owns the OpenTelemetry reader and exporter configuration. Reader collection
and export schedules are independent of the publisher's interval; applications configure each
schedule according to how frequently they need updated values and exported telemetry.

## Metric mapping

Each event's observation count is recorded as a counter under the event's instrument name. Its
accumulated magnitude is recorded as a gauge with the `_sum` suffix.

An event with a histogram also produces cumulative counters named with the `_bucket` suffix.
Each finite bucket carries an `le` attribute containing its inclusive upper boundary, and the
final `+Inf` bucket contains all observations. This representation preserves the cumulative
counts and sum exposed by `nm`, allowing a backend to display the distribution without requiring
the original observations.

The instrument name is the event name. Because the `_sum` and `_bucket` companions are derived
from it, an event name that already has the shape of a companion name — ending in `_sum` or
`_bucket`, optionally followed by further underscores — receives one additional trailing
underscore before its instruments are named. Every event name is accepted, and each event maps
to its own distinct set of instrument names, so no event's values are ever merged into another
event's instruments.

The configured meter name groups the instruments created by one publisher. Applications may
replace the default name when they need to distinguish this source from other OpenTelemetry
instrumentation.

## Continuous operation

Publication is an asynchronous, continuous operation. Applications run it on their async runtime
with a compatible `tick::Clock`; stopping the task stops further nm collection and instrument
recording. Export behavior after that point remains the responsibility of the configured
OpenTelemetry provider.
