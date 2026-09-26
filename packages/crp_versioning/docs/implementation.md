# Version assessment and application

This private component implements the [application design](../../cargo-release-plan/docs/design.md)
within the [application architecture](../../cargo-release-plan/docs/implementation.md).
It depends on workspace observations and diagnostics, never publication or the shell.

Anchors, released-content classification, reports and group/version decisions belong
here. Exact dependency edges are acquired by workspace; the owning versioning operation
derives group membership once and uses that model for its decisions. Inherited-value
change attribution is release policy, distinct from discovering inheritance syntax.

Plans, proposals, preparation, prospective workspaces, resolved captures, preview
and application stay together. Their captured-input and no-late-resolution invariants
must not be distributed across independently interpreted artifacts. Report and plan
producers own the schemas their consumers validate.

The typed workspace-check operation is shared by application dispatch and publication
candidate verification. It has no dependency on CLI command variants or publication
configuration. Production entry points acquire workspace observations; deterministic
inner operations receive acquired values and narrow ports.

Unit tests stay in process. Boundary integrations exercise real Git/Cargo and
filesystem behavior without depending on the application binary. Patch-rendering
benchmarks use an opt-in private driver. Versioning-only tests do not compile HTTP,
TLS, upload, native-delivery or application command parsing. Criterion's development
dependencies remain part of benchmark-enabled builds.
