# Native execution

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It executes validated native requests without choosing publication versions or receipts.

Historical source worktrees supply immutable build inputs and tracked toolchains.
The controller supplies Git objects and a shared target directory. Native verifies
the actual source, compiler host, selected package/bin and emitted executable before
staging archives and checksums. No registry or GitHub delivery policy belongs here.

Process groups, cancellation, deadlines and cleanup share one execution owner.
Preparing a source starts the first item's budget; its first build shares that budget.
Later builds start their own item budget. Publication receives the actual controller
directory and deadline for delivery, so upload and its recheck do not renew the budget.
Cleanup uses its separate conservative budget even after cancellation.

Repository-read authentication is explicitly scoped to source acquisition. Build
commands remove upload credentials and controller toolchain overrides. Diagnostic
destinations are supplied capabilities, not process-global state owned by producers.
Primary errors and independent termination, reaping, output-reader and worktree
cleanup failures remain attributable. A failed termination still attempts nonblocking
reaping; it cannot authorize an indefinite wait on a live child or unfinished reader.
The supervisor stops subsequent work when process-tree cleanup cannot be confirmed.
Cleanup does not depend on diagnostic delivery. Explicit finalization records its
results; native state also disposes an acquired worktree if an operation unwinds
before the batch can finalize it. Fallback diagnostics never replace that unwind.
Artifact staging and checksum errors retain their relevant paths separately from
invalid build-request errors, including the underlying filesystem cause when present.

Pure argument/source/artifact decisions are unit tested. Environment configuration,
real worktrees, toolchains, process trees and archive tools belong in integration
tests. Native I/O scheduling precedes last-chance watchdog timing; mutation testing
retains the watchdog disablement. The package does not depend on publication or CLI types.
