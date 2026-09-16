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

## Validation boundaries

Library tests exercise scenario validation, its use by parsed-input execution,
exit mapping and configuration writes. Validation and exit mapping need no
runtime or operating-system fixture. Configuration writes use isolated temporary
directories and cover replacement and filesystem failures without network access.
These filesystem tests are excluded from Miri, not from native mutation testing.

Seeded-object tests use small fixed scenarios and independent model expectations
to verify storage keys, compressed contents and reported byte totals. Single-object
and batch writes receive an in-memory storage operation and an explicit worker limit,
so they exercise shared accounting without filesystem access, OS parallelism discovery,
Git history or analysis. Invalid-input, injected storage-error and worker-panic cases
protect error propagation. These tests also run under Miri.

The seeding entry point discovers available parallelism and supplies the real
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
configuration side effects and exit mapping remain mutation targets.

The process-facing wrapper and real-I/O coordinator are excluded from line
coverage; the seeded binary assertions protect their complete execution.
The independently exercised validation, configuration writer and exit mapper
remain instrumented.
