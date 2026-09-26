# Diagnostic reporting

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It owns lazy diagnostic reporting and deterministic presentation, not release behavior.

Producers receive a reporting capability rather than acquiring a process stream.
The application selects stderr; private recording support observes the same operations
in memory. Disabled verbose reporting does not evaluate message-building closures.
The existing note prefix and best-effort note writes are distinct from unconditional
diagnostics and fallible child-output streaming. There is no timing or announcement channel.

Path quoting, count inflection and abbreviated labels keep diagnostic, error and
report presentation consistent. They do not validate filesystem identities, select
source commits or own artifact schemas. Full source/path identity belongs to workspace.
This package contains no filesystem, command, credential or release-policy operations.
