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

## Validation boundaries

Library tests exercise scenario validation, its use by parsed-input execution,
exit mapping and configuration writes. Validation and exit mapping need no
runtime or operating-system fixture. Configuration writes use isolated temporary
directories and cover replacement and filesystem failures without network access.
These filesystem tests are excluded from Miri, not from native mutation testing.

Successful end-to-end execution belongs to the seeded binary integration tests.
They verify analysis modes, findings and retained data without timing assertions.
They remain outside the library-only mutation build and execution selection.
The process-argument/runtime wrapper is a trivial forwarder and carries a
justified mutation exclusion; scenario decisions, parsed-input execution,
configuration side effects and exit mapping remain mutation targets.

The process-facing wrapper and real-I/O coordinator are excluded from line
coverage; the seeded binary assertions protect their complete execution.
The independently exercised validation, configuration writer, exit mapper and Git helpers
remain instrumented.
