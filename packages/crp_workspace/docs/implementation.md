# Workspace observations

This private component supports the [application design](../../cargo-release-plan/docs/design.md)
and is composed as described by its [implementation guide](../../cargo-release-plan/docs/implementation.md).
It owns Git/Cargo observations and the representations needed to interpret them.

Git subprocesses, manifest/member/dependency discovery, inherited-key acquisition,
tracked package contents, installation declaration graphs, lockfile parsing and
closure calculation, and source/path identity share this boundary.
Git and manifest interpretation may depend on each other inside
the package. Classification, version-group decisions and publication eligibility
belong to callers. Workspace observations retain validated exact dependency edges;
versioning derives their groups.

Git observations can resolve commits and test ancestor relationships without
rewriting history. Whether a descendant snapshot represents an anticipated squash
predecessor is versioning policy, not a workspace acquisition rule.
Ancestry exit-status interpretation stays in process: Git's positive and negative
answers are distinct from execution failures, including signal termination.
Historical manifest acquisition batches recorded blob identities through `git cat-file`.
Requests never encode paths as lines, and responses retain their exact byte lengths.
Callers decode only the manifests they interpret. Captured-input subprocesses feed
stdin concurrently with draining stdout and stderr, so large batches cannot deadlock
on opposing pipe buffers; child failures retain their diagnostics.

Path handling probes actual filesystem alias behavior rather than assuming case
sensitivity from the operating system. Dependency membership uses the lexical member
index first, then filesystem-resolved member identity, so caller path spelling does
not remove dependency edges. Shared artifact-file operations own path
resolution and atomic promotion, while callers own serialization and overwrite policy.
Symbolic links are unsupported: Git modes and direct file metadata identify released
links for rejection. General filesystem identity resolution still protects case and
short-name aliases and artifact write locations; it is not a symlink support protocol.
Repository-controlled display strings use the diagnostic component's presentation helpers.
The command boundary also supplies the fixed orchestration credential names shared by
native, compatibility and registry compilation; each adapter owns environment mutation.

## Disposable immutable observations

The cache storage subject owns location resolution through Cargo metadata, source/evidence
path admission, typed entry envelopes and atomic publication. Entry subjects own complete
keys and computation revisions. Exact keys are checked in addition to hashed filenames;
payload checksums detect accidental corruption, not hostile same-account modification.
An incompatible entry is a miss. Corruption and storage failures are diagnosed independently
of acquisition failures, and only successful acquisitions are published. Same-directory
temporary files and atomic replacement keep concurrent readers on complete entries.
The tool-owned directory ignores its untracked contents without excluding tracked source;
an existing ignore file is never replaced. Storage failure diagnostics are advisory and
reported once through the shared invocation store, including across prospective passes.
Source directories and workflow evidence cannot contain the cache.

Git's full recorded trees and raw parent-header facts are keyed by resolved object identity
and effective object interpretation. Their data can be shared across original and prospective
repositories; package projections remain repository/workspace scoped. Full-tree path and
mode/object indexes are constructed once and retained by the snapshot owner. Refs, traversal,
parent availability and shallow verdicts are always acquired freshly.

Replacement refs, replacement environment and graft contents are observed at each
classification boundary and invalidate invocation memory when they change. Histories with
replacement refs or grafts bypass persistent observations because their referenced-object
availability is not immutable. Neither credentials nor Git configuration values containing
credentials are stored. This is local disposable storage, not a remote trust protocol.

## Observation boundaries and tests

Pure parsing and graph tests remain in process. Real Git/Cargo/filesystem tests
belong to boundary integration targets, with hermetic Git identity/configuration.
Shared integration fixtures are opt-in private test support, not acquisition hidden
inside unit tests. `private-test-util` excludes repository creation, native filesystem
fixtures and I/O scheduling support. Inert Git constructors and the lockfile-closure
driver are ordinary public items in this private component: they only assemble captured
observations or call the existing in-process algorithm.

Observation adapters acquire process outputs, directory identities and file-type facts.
Their interpretation remains independent of native execution: status and optional-result
handling, first-parent endpoint selection, ancestor resolution, tracked-source eligibility
and configuration precedence consume captured values or narrow read callbacks. Metadata
projection shares those operations rather than duplicating publication and dependency
decisions in a test model. Only native forwarding is excluded from unit mutation testing;
boundary integrations exercise the actual commands and filesystem effects.
The shared repository fixture's Git and filesystem methods have individual
native-support mutation exclusions. I/O scheduling stays unit-tested, including
callback execution and rejection after a predecessor poisons its slot.
