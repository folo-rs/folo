# An ordinary release

This example assumes the packages are already published, their Trusted
Publishers are configured, and repository checks and publication are connected.
For a new package, use [first publication](first-publication.md) instead.

## Make and assess the change

Add a compatible public operation to `widget`, implement it in `widget_impl`,
and update the relevant tests and public documentation. The group starts at
`1.4.0`; `widget-cli` already has a pending `2.0.1` increment from `2.0.0`.

Run the copied `increment-versions` skill, or follow
[local planning](../integration/local-planning.md). Freeze the release baseline,
prepare resolution and assess dependency-first. The implementation change selects
the public library contract for external comparison.

Choose `nonbreaking` for the new library operation and assess the implementation
and binary dependency effects. Preview reveals the complete group and dependency
changes. Retain the binary's pending patch increment if it is sufficient.

## Present the complete release

Put a **Version/release plan** section in the PR description. Base it on the final
resolved expansion and current evidence, not a list of directly edited files:

| Package or group | Previous version | Proposed version | Change and reason |
| --- | --- | --- | --- |
| `widget`, `widget_impl`, `widget-fixtures` | `1.4.0` | `1.5.0` | Compatible public operation in `widget`; supporting implementation in `widget_impl`; helper moves for alignment only and is not published. |
| `widget-cli` | `2.0.0` | `2.0.1` | Assessed dependency update without a stronger CLI change; sufficient pending patch increment retained. |

Show previous versions at each publishable package's anchor. For a nonpublishable
helper, use its declared alignment starting point. If members start at different
versions, show each movement rather than hiding it in one group value.

Include:

- Every group member and ungrouped version target reached by the complete plan.
- Required dependent releases and requirement rewrites.
- Semantic reasons, including any level above the external checker's floor.
- Pending increments already present before the latest planning run.

Do not invent a consumer-facing change for alignment-only movement. If no
released-content or version changes exist, state that explicitly. An empty
newly generated plan does not by itself establish that nothing is pending.

New packages need a separate bootstrap/first-automated-version handoff. Keep
local artifact logs out of the PR's release explanation; the section describes
the final proposed release, not the sequence of planning attempts.

## Apply, validate and review

Apply the captured resolved artifact unchanged, verify locked metadata and run
the repository's required checks. Preserve the before/after evidence separately
from source.

Source, group, baseline or decision changes require reassessment and an updated
section. If another PR consumes the version, use
[release-branch movement recovery](recovery.md#release-branch-movement).

Human review approves the source and versions together. The skill does not
publish, and completing it is not merge authorization.

## Observe publication

After the authorized merge:

1. The workflow captures a manifest from the clean merged source, including
   all publishable packages at their declared versions.
2. Registry reconciliation uploads only missing versions.
3. GitHub reconciliation establishes package tags and binary releases.
4. Native jobs complete missing archive/checksum pairs from actual tag commits.
5. Outcomes and a failure report, when needed, identify remaining work.

The manifest omits `widget-fixtures` because it is not publishable, even though
the version plan aligned it.

Suppose the Linux assets complete but the Windows checksum is missing. Retry the
original run: registry versions remain present, complete Linux work is skipped,
and the Windows pair is repaired. No new version decision or package-list input
is needed.

Finish with [delivery verification](verification.md), not merely an observation
that one publication job was green.
