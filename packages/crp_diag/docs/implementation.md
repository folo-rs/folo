# Diagnostic reporting

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It owns lazy diagnostic reporting and deterministic presentation, not release behavior.

Producers receive a reporting capability rather than acquiring a process stream.
The application selects stderr; private recording support observes the same operations
in memory through an ordinary `NoteSink` implementation for a string-vector `RefCell`.
This small adapter needs neither a feature gate nor additional dependencies.
Disabled verbose reporting does not evaluate message-building closures.
Lazy construction avoids formatting disabled notes. Their tool prefix attributes
interleaved output, and note delivery is best-effort. Unconditional diagnostics retain
their explicit failure behavior; child-output streaming reports delivery errors only
after the pipe has been drained.

Path quoting, count inflection and abbreviated labels keep diagnostic, error and
report presentation consistent. They do not validate filesystem identities, select
source commits or own artifact schemas. Full source/path identity belongs to
[`crp_workspace`](../../crp_workspace/docs/implementation.md).
This package contains no filesystem, command, credential or release-policy operations.
Owned string and path forwarding is tested directly alongside the quoting transformations.
Only the concrete stderr write is excluded from library mutation testing: executable
integration tests observe that process stream, while destination and advisory-error behavior
remain covered with in-memory sinks.
