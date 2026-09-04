# alloc_tracker design

`alloc_tracker` measures the memory allocation activity of code under test. It is a
development tool for benchmarks and performance analysis, not a production facility.

Measurement requires the package's allocator to be installed as the global allocator. Code
that runs without it is not measured and reports zero activity.

## Sessions, operations and spans

A session owns the measurement of one program run and emits the results when it is
dropped. Within a session, an *operation* names a unit of work whose allocation cost is
being characterized, such as a single benchmarked function.

A *span* records one contiguous stretch of measurement for an operation. Each span is told
how many iterations of the operation it covers, so that a benchmark harness can amortize
measurement overhead across a whole sample rather than paying it per iteration. The
iteration count may be supplied at any point before the span is dropped, which allows
measuring work whose extent is only known once it is finished.

An operation accumulates every span recorded for it. Spans may nest and may be recorded
from several threads.

## Measurement scope

A span measures either thread scope or process scope.

Thread scope observes only the allocator activity of the thread that created the span. It
is the appropriate choice whenever the measured work stays on the calling thread, which
covers most benchmarks.

Process scope observes the allocator activity of every thread in the process. It is
necessary when the measured work is performed by other threads, and it accepts two costs
in exchange: it is more expensive to capture, and it attributes to the operation any
concurrent allocation by unrelated threads.

## Metrics

**Bytes per iteration** and **allocations per iteration** are rates: the totals observed
across all of the operation's spans, divided by the total iteration count. They describe
the cost of performing the operation once.

### Peak outstanding bytes

Peak outstanding bytes is not a rate. It is the largest amount of memory that the operation
was holding at any single moment, measured as a high-water mark relative to the memory
already outstanding when a span began. Reporting it as a whole-run maximum rather than a
per-iteration average is deliberate: the quantity of interest is how much memory has to
exist at once, and averaging that over iterations would describe nothing real. Where an
operation has several spans, the largest span watermark wins.

Peak outstanding bytes is reported only when every span of the operation could measure it.
Process-scope spans cannot, because a process-wide watermark would be perturbed by
unrelated threads to the point of meaninglessness, so an operation that contains even one
process-scope span reports no peak at all rather than a figure that silently describes
only part of the work.

#### Limits of the peak figure

Because the watermark is relative to the memory outstanding when the span began, an
operation that frees memory it did not allocate creates headroom that masks its own
subsequent allocations. Entering a span with a megabyte outstanding, releasing all of it,
and then allocating a kilobyte reports a peak of zero even though the operation held a
kilobyte of its own. Measuring an operation that primarily releases pre-existing memory is
therefore outside what this metric describes.

Deallocation is attributed to the thread that performs it. A thread-scope span covering
work that allocates on one thread and frees on another does not describe the amount of
memory live in the process.

## Reporting

A session emits its results in two forms when dropped: a human-readable table on stdout
with one row per operation, and one machine-readable JSON file per operation written into
the Cargo target directory. An operation with no peak figure renders as unavailable in the
table and omits the field from JSON.

Results can also be taken from a session as a report value, which is independent of the
session and may be moved between threads, merged with other reports, and inspected
programmatically.
