# Native execution

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It executes validated native inputs without choosing publication versions or outcomes.

The **controller checkout** is the invocation's Git checkout. It identifies the
selected Cargo workspace and supplies Git objects and a configured shared target
directory. A **build request** identifies one package/version and binary target,
its immutable source commit, and the archive basename. It is an execution input,
not a publication manifest, platform batch or outcome.

Historical source worktrees supply immutable build inputs and tracked toolchains.
Native uses the controller checkout to create those source worktrees for build requests.
It verifies the actual source, compiler host, selected package and binary target, and executable before
staging archives and checksums. No registry or GitHub delivery policy belongs here.

ZIP creation uses the `zip` crate with only Deflate and the explicitly selected
safe-Rust `miniz_oxide` backend through `flate2`.
Staging and the ZIP writer stream executable bytes through the same bounded-copy
checkpoints, retaining the root name
and Unix permissions. A fixed ZIP timestamp avoids incidental source-file timestamp
variation; it is not a promise of byte-identical compression across tool versions.
ZIP64 represents large entries without a separate archive format. Staged executables
are promoted from an owned temporary file only after copying and flushing succeed.
The archive is promoted from an owned temporary file only after explicit
finalization and flushing. A failed writer cannot leave a plausible partial ZIP
at the final asset path, and promotion does not replace an existing asset.

The same item deadline and cancellation flag are checked during the staging copy,
between ZIP input buffers, around finalization, and while hashing the finished archive. Interruption
and I/O errors fail packaging before the checksum pair is eligible for upload.
These checks are cooperative around file I/O, not preemption of a blocked system call.
The in-memory writer and error boundaries are unit-tested; .NET independently
reads and extracts produced archives in native integration tests. Neither the
application nor its setup needs an external archiver.

Process groups, cancellation, deadlines and cleanup share one execution owner.
Preparing a source starts the first item's budget; its first build shares that budget.
Later builds start their own item budget. Publication receives the actual controller
directory and deadline for delivery, so upload and its recheck do not renew the budget.
Cleanup uses its separate conservative budget even after cancellation.

Toolchain manifests are committed regular files contained in the immutable worktree.
Git entry modes are checked even when a host materializes symlinks as ordinary files.
Checkout skips automatic Git LFS smudging; native execution adds no LFS download or
authentication phase. Any materialization a build needs remains part of its normal prerequisites.

Repository-read authentication is explicitly scoped to source acquisition. Build
commands remove upload credentials, including Cargo registry-token aliases, and
launcher toolchain overrides while retaining build homes and intentional configuration. Diagnostic
destinations are supplied capabilities, not process-global state owned by producers.
Normal stderr mirroring retains a delivery failure but continues reading through EOF;
the completed process result, available text and delivery failure are resolved together.
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
real worktrees, toolchains, process trees and archive files belong in integration
tests. Native I/O scheduling precedes last-chance watchdog timing; mutation testing
retains the watchdog disablement. The package does not depend on publication or CLI types.

Credential-name decisions retain interpreter coverage independently of Command's
native environment-key comparison. Windows Command environment assertions run
natively because the interpreter does not implement that comparison.
Library mutation testing excludes only the signal-state forwarders and native
file-support adapters whose callers require integration execution. Literal arguments,
identifier admission, interruption decisions and ZIP64 header reservation remain
in-process behavior coverage; large declared sizes require no large input allocation.

The low/high in-memory archive benchmark measures ZIP/Deflate work independently
of source acquisition and filesystem noise. Its driver and file-boundary interruption
adapters are ordinary public functions in this private component. They forward to the
production copy/archive operations without fixture generation or extra dependencies,
so they need no private feature. The in-memory writer specialization is shared with
unit coverage rather than introducing a separate benchmark archive implementation.
