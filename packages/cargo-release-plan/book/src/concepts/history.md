# Release history and package status

The **release branch** is the branch whose reviewed versions your publication
workflow delivers. Its name is repository policy, not necessarily `main` and not
necessarily the target of the pull request being assessed.

A **release baseline** is one frozen Git commit selecting the release-branch
history available to an assessment. Within that history, a package's **anchor**
is its newest first-parent commit where the parsed package version changed.
The baseline selects history; the anchor supplies that package's comparison
version and content.

```text
release branch

A ---- B ---- C ---- D ---- E   <- frozen release baseline
       ^           ^
       |           +-- widget-cli anchor: 2.0.0
       +-------------- widget and widget_impl anchors: 1.4.0

assessed work tree
  widget and widget_impl: compare with B
  widget-cli:             compare with D
```

An unrelated change at `E` does not reset these anchors. Comparing only the
baseline's files with the work tree would miss accumulated changes since a
package's own version decision.

First-parent history follows the release line rather than the commits made while
authoring a topic branch. For a merge commit, the anchor is where the version
reaches that line. Reformatting a version declaration does not change its parsed
value and does not create an anchor. Adding a package does.

Full history is required. An anchor hidden by a shallow or truncated checkout
cannot establish version readiness.

## What the assessment says

| Status in `report.json` | Meaning |
| --- | --- |
| `pending-release` | The declared version is above its anchor, or the package is preparing its first release. |
| `needs-increment` | Released content changed without advancing the version beyond its anchor. |
| `unchanged` | Released content and version still match the anchor. |

`needs-increment` fails the release-readiness check. Group consistency, dependency
requirements and public-dependency compatibility are independent checks. A version
below its anchor is an error, not another status.

Pending increments remain valid while a contribution develops: all of its
released content ships under that version. Authors must still reassess whether
new changes require a greater semantic level. Rerunning planning does not require
another increment when the pending one is sufficient.

Packages with `publish = false` receive no release status, but can participate in
[version alignment](versions.md#version-groups).

## Git state is not upload state

Suppose `widget` version `1.5.0` merges. That merge becomes its anchor, so an
assessment of the merged source can report `unchanged` even while its registry
upload is waiting or has failed.

Neither `pending-release` nor `unchanged` proves that crates.io, a tag or an
archive exists. Publication checks exact remote versions and assets separately.
It therefore considers **every publishable package in its selected source**, not
only the report's pending-release entries.

The release baseline is also not an external API checker's comparison version.
A compatibility checker commonly compares against a published crate version.
That is useful API evidence, but it does not select Git history for this model.

## Select the right baseline

| Context | Baseline |
| --- | --- |
| Local branch or ordinary PR | Freeze the actual release-branch tip once. |
| Stacked PR | Still use the release branch, not the unreleased parent PR. |
| Merge queue | Use the release-branch base commit of the tested queue candidate. |
| Merged-source check or publication preparation | Use the pinned merged source as the history boundary. |

Pass `--base` explicitly in assessment automation. Without it, ordinary
assessment uses the default branch advertised by `origin`, falling back to
`origin/main`. Publication configuration does not silently change that default.
Use `release-context` to fetch the configured repository's actual release branch
and obtain its frozen `release_base`, then pass that commit to assessment.
An explicit `release-context --base` instead uses an already tested boundary.

Two concurrent PRs can choose the same next version without a textual merge
conflict. A required merge queue, or required up-to-date checks against the latest
release branch, makes the later contribution reassess that version. A queue
combining both contributions into one tested merge can instead release them
together under one version.

If the release branch advances during planning, refresh the assessment rather
than assigning old evidence a new baseline. See
[stale-plan recovery](../operations/recovery.md#release-branch-movement).

Maintenance series can use their own release branches and baselines. Version
monotonicity is relative to the selected release line; registry identity still
has to be reconciled before publication.
