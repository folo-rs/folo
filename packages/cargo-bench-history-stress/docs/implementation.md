# Stress harness implementation

## Boundaries

The binary delegates to the library's process entry point. That wrapper owns
process-argument parsing and the Tokio runtime; parsed-input execution and
result-to-exit-code mapping are independently callable within the library.
Scenario validation is synchronous and precedes resource creation.

Repository construction, object generation, storage lifecycle, measurement and
reporting remain separate components. Measurement enters the public
`cargo-bench-history` API, while generation reproduces its storage write format.
The [design](design.md) defines the harness's execution and cleanup contract.

The execution coordinator keeps the provisioned target alive across seeding,
upload and measured analysis. It awaits cleanup before propagating either the
execution result or cleanup result. Temporary repository and workspace owners
remain alive until those operations finish.

## Repository construction

The generated repository and stored benchmark objects must describe the same commit
topology so the measured analysis follows its ordinary Git and storage paths.
Repository construction writes the dated branch history through Git fast-import,
reads Git's exported marks to obtain the assigned commit IDs, and restores a clean
main-branch checkout. Storage seeding uses those IDs rather than independently
inventing identities. Subprocess launch, input, completion, and unsuccessful exits
are errors; an absent marks file is not interchangeable with an empty one.

Library tests exercise stream construction, marks parsing, branch identity resolution
and exit-status decisions entirely in memory. Both Git invocation adapters delegate
completion decisions to the same helper; file acquisition delegates marks interpretation
to the parser. Their I/O, pipe handling and delegation remain covered by the seeded
binary integration scenarios, outside library mutation testing. Only these acquisition
adapters carry justified mutation exclusions; their parsing and decision logic remains
instrumented and mutation-tested without a runtime, filesystem or subprocess fixture.

## Configuration writes

Storage targets render their backend configuration without performing I/O. The writer
accepts those rendered contents, derives the same destination used by measurement,
awaits parent creation before writing, and forwards failures with the attempted path.
Small asynchronous operations keep that sequencing and error reporting independent of
Tokio and the filesystem. The execution coordinator logs the returned destination only
after a successful write.

The nonpublished library exposes the filesystem writer directly to Cargo integration
tests. Those tests cover parent creation, replacement with local and Azure configuration
contents, truncation and filesystem failures without constructing storage staging areas
or cloud resources. Only the thin Tokio delegation carries a mutation exclusion;
configuration rendering, destination selection, sequencing and error forwarding remain
library mutation targets.

## Measurement

Each analysis attempt owns a fresh process-CPU measurement session and pairs its
recorded duration with that attempt's wall time. The real-system coordinator passes
the report's operation durations to an in-memory selector. Selection preserves the
first operation's duration exactly, including zero, and returns zero for an empty
report. Fastest-attempt selection carries the corresponding CPU duration with the
wall time and keeps the incumbent on ties.

Library tests supply controlled durations to exercise empty reports, measured zero,
exact nonzero forwarding and paired attempt selection without sampling OS clocks.
The seeded binary integration tests exercise real measurement and report acquisition
without asserting elapsed times. No published clock or report-construction testing
API is needed at this boundary.

CPU-efficiency accounting uses the quota-respecting processor count from `many_cpus`,
matching the capacity supplied to production analysis even across Windows processor groups.
This denominator describes the process-wide processor budget, not the runtime's thread count
or a thread's soft-affinity mask; the harness does not change either.

## Validation boundaries

Library tests exercise scenario validation, its use by parsed-input execution,
exit mapping, configuration serialization and write orchestration in memory.
Configuration callbacks record requested paths and contents and inject errors without
a runtime, filesystem or storage target. These cases run under Miri as well as native
mutation testing; real filesystem tests belong to integration targets.

Seeded-object tests use small fixed scenarios and independent model expectations
to verify storage keys, compressed contents and reported byte totals. Single-object
and batch writes receive an in-memory storage operation and an explicit worker limit,
so they exercise shared accounting without filesystem access, OS parallelism discovery,
Git history or analysis. Invalid-input, injected storage-error and worker-panic cases
protect error propagation. Miri covers each object format with a reduced metric
fixture and shared accounting with compact sidecars, keeping repeated compression
within the interpreter workload budget without changing production behavior.

The seeding entry point uses `many_cpus`'s nonempty, quota-respecting processor set
for its worker count and supplies the real
filesystem adapter. Object construction and encoding precede storage; only a
successful write contributes its compressed length to the total. The adapter creates
parent directories and writes the supplied bytes. This real-I/O boundary is covered by
the existing retained-data binary integration test rather than library mutation or
line coverage; generation, dispatch, error propagation and accounting remain unit
mutation targets.

Successful end-to-end execution belongs to the seeded binary integration tests.
They verify analysis modes, findings and retained data without timing assertions.
They remain outside the library-only mutation build and execution selection.
The process-argument/runtime wrapper is a trivial forwarder and carries a
justified mutation exclusion; scenario decisions, parsed-input execution,
configuration decisions and exit mapping remain mutation targets.

The process-facing wrapper and real-I/O coordinator are excluded from line
coverage; the seeded binary assertions protect their complete execution.
The independently exercised validation, configuration writer, exit mapper and Git helpers
remain instrumented.
