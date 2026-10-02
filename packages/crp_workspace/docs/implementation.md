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
