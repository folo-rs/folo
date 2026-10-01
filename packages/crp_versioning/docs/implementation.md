# Version assessment and application

This private component implements the [application design](../../cargo-release-plan/docs/design.md)
within the [application architecture](../../cargo-release-plan/docs/implementation.md).
It depends on workspace observations and diagnostics, never publication or the shell.

Anchors, released-content classification, reports and group/version decisions belong
here. Exact dependency edges are acquired by workspace; the owning versioning operation
derives group membership once and uses that model for its decisions. Inherited-value
change attribution is release policy, distinct from discovering inheritance syntax.

Plans, proposals, preparation, prospective workspaces, resolved state, preview
and application stay together. Their captured-input and no-late-resolution invariants
must not be distributed across independently interpreted artifacts. Report and plan
producers own the schemas their consumers validate.

The skill's semantic decisions use `impact`; a proposal's mechanical version choices
use `bump` or an explicit version. Proposal generation translates semantic meaning
into version arithmetic before preview expands groups and dependency consequences
to a fixed point. There is no separate expansion operation. The expanded marker
binds a preview to its complete explicit package set, while captured source and
file effects make that preview applicable. Both real and dry-run application
require this captured state; neither interprets a proposal as live manifest edits.

## Anticipated squash predecessors

Assessment keeps the actual release-history commit separate from an optional final
merge-target commit. A target equal to or already an ancestor of release history
normalizes to no projected target. Otherwise it must descend from that history;
divergent refs require refresh/rebase rather than an invented merged timeline.
`history::resolve_merge_target` owns this local ref-resolution and ancestry rule
for both assessment capture and publication-context callers; it performs no fetch.
The supplied ref remains captured: verification rejects movement that changes
the effective target, while movement wholly within committed release history
does not introduce a new predecessor.

Ref resolution and ancestry acquisition enter through narrow callbacks. The same
in-process protocol admits targets and revalidates retained refs, including acquisition
failures, independently changed identities and transitions into or out of committed
history. Native Git adapters retain boundary coverage. Group exemptions require
absence from both actual history and the anticipated predecessor; publication-disabled
members still bind the group.

The target contributes one anticipated predecessor snapshot, not its intermediate
commits. A package first published by the target, or declaring a higher version
there, anchors to that final snapshot. A package retaining its history version
keeps its actual historical version-change anchor. This preserves catch-up
obligations from unversioned commits in real release history or in the parent.
Group membership, dependency graphs and content/lockfile comparison remain the
ordinary classifier's responsibilities.

Captured inputs retain `release_history` and `release_history_revision`, plus
`merge_target` for a distinct predecessor and `merge_target_revision` whenever supplied.
Live verification resolves
the original refs and rejects movement. Prospective and retained workspaces use
the frozen commits, independently of the clone's ref names; after candidate
verification, the original captured refs are checked again at the source repository.
Preparation retains
the context across lockfile refresh; every fixed-point classification uses it.

Reports expose the resolved history and optional target identities. Generated
proposals carry that same context through preview, which rejects
bound proposals from another prepared context. Resolved plans include both the context
and captured inputs; verification and application enforce their agreement.
The shared report/plan/prepared schema is defined by `plan::SCHEMA_VERSION`;
semantic decision documents keep their separate `DECISION_SCHEMA_VERSION`.
`ReportFile::anticipated_parent_anchor` exposes the existing anchor commit and version
for packages whose predecessor is the final parent snapshot.
The classifier validates that a distinct target descends from the
release history, so its commit cannot also be an anchor in that actual history.
External compatibility execution uses this immutable source as its comparison root;
a registry release is not a substitute for an anticipated, still-unpublished predecessor.

## Shared operation and tests

The typed workspace-check operation is shared by application dispatch and publication
candidate verification. It has no dependency on CLI command variants or publication
configuration. Production entry points acquire workspace observations; deterministic
inner operations receive acquired values and narrow ports.

Unit tests stay in process. Boundary integrations exercise real Git/Cargo and
filesystem behavior without depending on the application binary. Patch-rendering
benchmarks use an opt-in private driver. Versioning-only tests compile only this
component and its intended lower-level dependencies. Criterion's development
dependencies remain part of benchmark-enabled builds.

Released-content comparison consumes acquired archive paths, object identities and modes.
It requests bytes lazily only for content changes, independently of mode-only changes.
Historical member discovery reads recorded paths through the same cache and membership
logic whether observations come from Git or an in-process fixture.

Captured-source traversal, artifact admission and prospective evidence ownership use
injected observations and effects. This keeps transitive membership, original/final
fingerprints, retained isolation, foreign-owner rejection and marker invalidation within
unit mutation coverage. Native adapters retain integration coverage for subprocess
arguments, real aliases, atomic file promotion and workspace lifetime.
