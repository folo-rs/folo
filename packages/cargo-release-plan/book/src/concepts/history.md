# Release history and package status

Version assessment answers: **which changes belong to this pull request's release,
and which versions must move to cover them?**

It needs the repository's existing release history and, for a stacked PR, the
final state of the parent that will merge first. These are different inputs.

## Existing release history

The **release branch** is the branch whose merged versions your publication
workflow delivers. **Release history** means that branch up to a selected commit.
The commit ID fixes which history the report, preview and final check use.

For each package, the tool looks backward through first-parent history for the
most recent commit that changed its parsed version. This is the package's
**anchor**: its comparison version and content.

```text
release branch

A ---- B ---- C ---- D ---- E   <- selected release-history commit
       ^           ^
       |           +-- widget-cli anchor: 2.0.0
       +-------------- widget anchor: 1.4.0
```

An unrelated commit at `E` does not change either package's comparison point.
If somebody changed `widget` after `B` without increasing its version, those
changes still need a release. Comparing only with `E` would hide that unfinished
work. Anchors allow a repository to catch up after migration or manual changes.

Reformatting a version string does not create an anchor; changing its parsed value
or adding the package does. Full history is needed to find the applicable anchor.

## The current pull request's target

The **merge target** is the commit the pull request intends to merge into.
For an ordinary PR targeting the release branch, it is already release history.
For a stacked PR, it can be the final commit of an unmerged parent PR.

The parent may increase a package to `1.5.0` and then receive more fixes.
Its final content belongs to its `1.5.0` release, regardless of which intermediate
commit edited the version. The child compares with that final parent snapshot.
If the child changes released content too, it needs a version above `1.5.0`.
It cannot reuse the parent's increment merely because publication has not happened yet.

Packages retaining their release-history versions keep their historical anchors.
Unversioned work in existing history therefore remains visible even when an
unrelated parent PR is present.

The managed workflow uses **squash merges**: one merged PR contributes one commit
containing its final content and final versions. The anticipated parent state
then agrees with the state recorded in release history. Fast-forwarding the
parent's intermediate commits is not the supported publication workflow.
The history can still contain older or manually created commits that omitted
version planning; the anchor comparison finds their catch-up work.

Each PR is assessed for its own target. Neither the skill nor the version decision
assumes that a merge queue will merge a stack together.

## What the assessment says

**Released content** is what matters to consumers of a Cargo package, including
its packaging inputs and an installable binary's locked dependencies. The
[next chapter](released-content.md) explains that boundary.

| Status in `report.json` | Meaning |
| --- | --- |
| `pending-release` | The working-tree version is above the comparison version, or this PR introduces the package. |
| `needs-increment` | Released content changed without advancing its comparison version. |
| `unchanged` | Content and version match the comparison state. |

A pending increment already covers some work in this PR. Reassess all of that
work, retaining the increment if sufficient and raising it if a stronger semantic
decision requires it. Rerunning planning does not itself require another increment.

`needs-increment` fails version readiness. Group consistency and dependency
requirements are checked separately; so are
[public dependency compatibility obligations](versions.md#private-apis-and-public-dependencies).
A version below the comparison version is an error.

Packages with `publish = false` receive no release status, but can participate in
[version alignment](versions.md#version-groups).

## Select and refresh the inputs

`release-context --config <path>` fetches the configured repository's release
branch and returns `release_history`. For a stacked PR, also supply
`--merge-target <parent-ref>`. The returned `merge_target` is either its full commit
ID or null when the target is already in release history. Pass both returned
values to assessment rather than fetching history independently in each stage.

`--release-history <commit>` explicitly selects known release history; `--base`
is a compatibility alias. Without an explicit history, direct assessment commands
use `origin`'s recorded default branch, falling back to `origin/main`.
The configuration-aware context command is preferable for automation.

Before applying a plan, refresh history and the original target ref. If either
changed, reassess. Another PR may have consumed a proposed version, or the parent
may have gained changes that alter the child's comparison. Do not assign old
evidence to new commits. See [recovery](../operations/recovery.md#release-branch-movement).

Merged-source validation uses the pinned merged source as release history and
has no anticipated parent. A maintenance series can use its own release branch.

## Git state is not upload state

After a version-changing PR merges, its package can be `unchanged` against its
new anchor even if registry upload has failed. Git versions establish the
assessment model, not delivery completion.

Publication separately checks exact registry versions, tags and archives. It
considers every publishable package in its selected source, not just entries that
a pre-merge report called `pending-release`.

The API compatibility checker also has a comparison source. Existing releases
normally use a published crate; a child may instead need the anticipated parent's
source so that a newly added parent API cannot disappear unnoticed in the child.
