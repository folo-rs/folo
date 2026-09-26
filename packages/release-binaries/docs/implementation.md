# Release binary batches

The release workflow builds this nonpublished compatibility executable from its
event checkout. Its library delegates to `crp_impl::publication::binaries`, which
owns native planning, execution and unit tests. The workflow's behavioral contract belongs to
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
The selected Cargo workspace can be nested inside the Git repository. Source
commands use that same repository-relative workspace, and rustup reads its tracked
toolchain from within the source repository. No Folo source-toolchain script is
required by the installed application.

Native `zip` or standalone 7-Zip (`7za`) packages each immediately staged executable.
`just install-tools` installs and verifies these prerequisites on every platform. SHA-256 sidecars
name the archive; `gh release upload --clobber` repairs both members of an incomplete
pair. Only GitHub subprocesses receive the upload token. Worktree materialization
retains checkout's no-LFS-download behavior.

Independent item failures do not suppress remaining items, but any failed item
fails the batch. Worktree cleanup failures are retained alongside operation errors
for items in that source group, not releases already complete during refresh.
If Git removal and directory cleanup both fail, the outcome retains both diagnostics.
The item deadline bounds source preparation, compilation and publication together.
The command adapter terminates an owned process tree before returning a deadline
failure. Signal handling marks the batch cancelled, terminates active child groups and
prevents later uploads. Cleanup retains its own bounded deadline. The workflow owns the
overall job deadline.

`--no-upload` stages the exact requested items without querying release completeness
or writing to GitHub. It retains all source/build/archive checks and needs no upload
credential. Integration fixtures and the three-platform validation smoke exercise this
path. A native fake GitHub client also exercises query/upload arguments, credential
isolation, partial-publication recovery and summary reporting without network access.
No test creates or modifies a production release.

## Tests

Pure protocol validation, grouping, completeness, artifact selection and batch
transitions are unit tests in `crp_impl`. Git, Cargo, process and archive interactions
are integration tests. Native adapters have narrow mutation exclusions; the
decisions they execute remain covered in process.
Source fixtures include exact-object fetching from a local origin whose tip has advanced,
mixed source commits and native-host rejection. Cleanup failure coverage preserves
successful publication outcomes while retaining the cleanup diagnostic and failing the job.
