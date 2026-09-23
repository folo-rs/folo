# Release binary batches

The release workflow builds this nonpublished controller utility from its event
checkout. The workflow's behavioral contract belongs to
[release automation](../../../.github/workflows/design.md#publication-and-recovery).
Historical source worktrees never supply the orchestration executable.

## Planning and execution

PowerShell supplies discovered binary packages, their immutable tag commits and
the existing target-to-runner mapping. `plan` reads release assets, selects incomplete
archive/checksum pairs and groups them by target. `run` consumes one frozen batch
and refreshes completeness using the same predicate. Only uploaded assets count;
failed queries never authorize a skip. The JSON is private workflow coordination,
not a supported external CLI.

Each source commit has a separate temporary worktree. Cargo runs inside that
worktree with its pinned compiler, configuration and locked dependencies. Separate
package builds preserve feature selection. The absolute controller target directory
is shared across source builds so the existing cache and Cargo's fingerprints
govern reuse. The controller executable is built before these overrides apply.

Native `zip` or standalone 7-Zip (`7za`) packages each immediately staged executable.
`just install-tools` installs and verifies these prerequisites on every platform. SHA-256 sidecars
name the archive; `gh release upload --clobber` repairs both members of an incomplete
pair. Only GitHub subprocesses receive the upload token. Worktree materialization
retains checkout's no-LFS-download behavior.

Independent item failures do not suppress remaining items, but any failed item
fails the batch. Worktree cleanup failures are retained alongside operation errors.
The item deadline bounds source preparation, compilation and publication together.
The command adapter terminates an owned process tree before returning a deadline
failure. Signal handling marks the batch cancelled, terminates active child groups and
prevents later uploads. Cleanup retains its own bounded deadline. The workflow owns the
overall job deadline.

`--no-upload` stages the exact requested items without querying release completeness
or writing to GitHub. It retains all source/build/archive checks and needs no upload
credential. Integration fixtures and the three-platform validation smoke use this
path; no test creates or modifies a production release.

## Tests

Pure protocol validation, grouping, completeness, artifact selection and batch
transitions are library unit tests. Git, Cargo, process and archive interactions
are integration tests. Native adapters have narrow mutation exclusions; the
decisions they execute remain covered in process.
