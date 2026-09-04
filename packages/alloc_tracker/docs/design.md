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
from several threads. Overlapping spans on one thread must be dropped in reverse order of
creation, which holding each in a scoped binding achieves naturally.

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

Every metric is a per-iteration figure, estimated across all of the operation's spans in a
way that weights each span by the square of its iteration count. Benchmark harnesses run
short warmup batches before settling into long steady-state ones, and the weighting makes
those short batches contribute almost nothing without anyone having to identify which they
were.

**Bytes per iteration** and **allocations per iteration** describe the cost of performing
the operation once.

### Peak outstanding bytes

Peak outstanding bytes is the amount of memory one iteration of the operation holds at its
high-water moment, measured relative to the memory already outstanding when a span began.
It answers how much memory has to exist at once, which the cumulative byte count cannot:
an operation that takes and releases a buffer a thousand times and one that holds a
thousand buffers allocate the same total.

The estimate assumes that every iteration within a measured batch reaches the same peak. A
batch's watermark is therefore read as that per-iteration peak directly, which is what
allows batches of different sizes to be combined at all, and what allows a short warmup
batch with an anomalous watermark to be outweighed by the steady state.

Peak outstanding bytes is reported only when every span of the operation could measure it.
Process-scope spans cannot, because a process-wide watermark would be perturbed by
unrelated threads to the point of meaninglessness, so an operation that contains even one
process-scope span reports no peak at all rather than a figure that silently describes
only part of the work.

#### Limits of the peak figure

An operation that accumulates memory across the iterations of a batch — one whose watermark
grows with the batch size rather than staying level — has no per-iteration peak for this
metric to report. It reports a figure that scales with whatever iteration counts the
harness chose, and is not comparable between runs.

Span watermarks are averaged, not summed. An operation measured concurrently on several
threads reports what a typical one of them held, not the total held across all of them at
once.

The measured quantity is the memory requested through the allocator, sampled at the
boundaries of allocator calls. Memory an allocator transiently holds inside a call — a
reallocation that copies into a new block before releasing the old one, for example — is
not part of it.

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
