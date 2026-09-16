# Processor time measurement

`all_the_time` measures processor consumption for named benchmark operations.
Processor time counts execution rather than wall-clock waiting. A thread
measurement covers the current thread; a process measurement covers all threads
in the process.

## Measurement spans

A span captures work over its lifetime and records it when dropped. The caller
supplies the number of iterations covered by that work. Spans for the same
operation accumulate their full processor-time totals and iteration counts.
An active span does not contribute to a report until it closes. Work interrupted
by unwinding does not contribute a measurement.

Zero iterations are permitted for workloads that could not complete. They are
distinct from completed iterations that consumed no measurable processor time.

## Reports

A report is a detached snapshot of operation measurements. Reports can cross
threads and merge operations with matching names.

The primary per-iteration figure is a through-origin processor-time estimate
weighted by squared iteration counts rather than a pooled arithmetic mean.
This reduces the influence of low-iteration warmup spans. Totals remain
available independently of this estimate.

An operation without spans has no per-iteration estimate or statistics. Recorded
spans covering no iterations have statistics with an undefined rate and no
finite duration estimate. Completed iterations with no measured processor time
have a valid zero duration estimate. These states remain distinguishable through
the report API.

## Session output

Dropping a session with completed iterations emits a human-readable summary to stdout
and machine-readable JSON files under the Cargo target directory. Either
destination can be disabled independently. Sessions without completed iterations and
sessions dropped during unwinding emit nothing.

Zero measured processor time does not suppress completed iterations. Explicit
JSON report output can also represent zero-iteration spans; registered but
unmeasured operations have no JSON file. File names are sanitized; collisions are
rejected before any output files
are written. Output failures are surfaced rather than silently losing results.
Human-readable report wording and layout are not contractual.
