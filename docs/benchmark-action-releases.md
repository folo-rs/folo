# Benchmark action releases

This chapter owns the repository-specific release coordination between `folo-rs/folo`
and `folo-rs/cargo-bench-history-action`. The action selects exact tool versions, but its
own version and publication are separate from Cargo package versioning.

## Meta

* **Open this when**: completing monorepo version planning, preparing or refreshing a paired
  action PR, or following its package and archive publication dependencies.
* **Cross-links**: [`release-versioning.md`](release-versioning.md) (general crate version
  planning), [`release-automation.md`](release-automation.md) (monorepo publication),
  [`git-workflow.md`](git-workflow.md#versionrelease-plan-section) (general PR presentation),
  [action design](../packages/cargo-bench-history/docs/reusable-action.md#81-releasing-the-action-operator-flow)
  (installation and action release mechanics).

Run the [`pair-benchmark-action-release` skill](../.github/skills/pair-benchmark-action-release/SKILL.md)
after the general [`increment-versions` skill](../.github/skills/increment-versions/SKILL.md),
including after reassessment. The extension consumes the verified complete release plan;
it does not change the general skill or its crate-versioning decisions.

## Fast relevance check

After version planning, start with one command using the final verified report:

```powershell
just benchmark-action-pairing-needed 'C:\release-evidence\verified\report.json'
```

It prints only `true` or `false` on stdout. `false` ends action coordination immediately:
do not inspect action PRs, clone another checkout or read the remaining action-release procedure.
`true` means a pinned tool is in the pending release set, including retained increments and
dependency/group effects; continue with the pairing skill. It does not mean the action pins
necessarily still need editing.

The nonpublished helper reads the authoritative tool names from the action repository's
`release.json`, not a second maintained list. No pending releases means no manifest lookup at all.
Otherwise it fetches only that small file through the existing GitHub CLI authentication.
Malformed evidence and lookup failures are errors, never `false`.

For an initial action bootstrap or a proposed manifest not yet on its default branch, supply
the local manifest explicitly:

```powershell
just benchmark-action-pairing-needed 'C:\release-evidence\verified\report.json' 'C:\Source\cargo-bench-history-action\release.json'
```

## Release scope and version decisions

Whenever a monorepo PR moves a tool version pinned by the action, its author also creates or
updates a linked action PR with the corresponding exact pins and an appropriate independent
action-version increment. A tool-pin-only change still needs an action release.
This applies regardless of the reason for the tool's movement: source changes, dependency or
group effects, version-only increments, and first publication all count. Inspect the complete
verified pending release set, including retained increments, not only a newly applied plan.

The action's release manifest defines the tool set, including test-only tools. It covers
`cargo-bench-history`, `cargo-bench-history-github`, `cargo-bench-history-faker`, and the
`cargo-detect-package` dependency used by workflow scope selection. Check the actual manifest
for additional pins; do not maintain a second machine-readable list in this repository.
Private implementation packages matter through the pinned binary version they move, not
through independent action pins. The action repository's own release instructions determine
its version decision; monorepo tool versions do not determine the action's version number.

## Paired PR presentation

Create or update the action PR alongside the monorepo PR and cross-link both descriptions,
preserving their existing content and **Version/release plan** sections, following the
[general presentation contract](git-workflow.md#versionrelease-plan-section).
Include the linked action PR and its publication dependency in the monorepo's
**Version/release plan** section.

Reuse an existing paired PR when it already covers the same release work. Refresh its pins,
action-version decision and both descriptions whenever the final monorepo plan changes,
including after release-baseline recovery. If version planning precedes PR creation, retain
the pairing and reciprocal-link obligation in the local handoff and fulfill it when creating
the monorepo PR. An unavailable action checkout or missing repository permission is an
explicit handoff blocker, not permission to omit the paired change.

Describe pending tool publication as the action PR's merge dependency, not as a defect to
bypass. Creating the required paired PR is a monorepo readiness obligation; successful
installation gates the action PR's merge, not the monorepo merge that starts its publication.
Report access, pairing, verification and publication blockers in the handoff. Neither the
pairing obligation nor completing its skill authorizes merging or publishing either repository.

## Publication and required installation gate

Merge order is monorepo first, action second. Monorepo merge starts asynchronous registry and
prebuilt-binary publication; it does not prove that the selected versions are installable.
The action repository's required `install-tools` check must really install each exact manifest
package through `install` with its published lockfile and its promised prebuilt archives
through `binstall` with source fallback disabled, across the supported targets. It verifies
the executable versions and exercises the command contracts used by the action and workflows.

Availability runs bypass the installed-binary cache and use isolated installation roots.
Source fallback, an existing binary cache, or a source-checkout build is not proof of
published-binary availability. Early failure while publication is pending is expected
**and still merge-blocking for the action**, even when only an archive is missing.
This is a required merge check: the action PR cannot merge while it is pending or failed,
even when unrelated advisory checks would not delay a merge.

External publication does not rerun a failed GitHub check. The paired PR's author follows the
monorepo release and explicitly reruns the action check after its required packages and archives
are available, then verifies the actual result for the current manifest. No cross-repository
dispatch service or stored credential is needed. An action merge triggers its own release
workflow, which rechecks availability before creating the action release and moving the
floating major tag. A later successful run never justifies waiving the gate on an untested
manifest.
