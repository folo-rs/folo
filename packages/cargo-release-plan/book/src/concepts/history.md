# Which release are we changing?

Before choosing a new version, the skill needs to know **which package version
and content this pull request follows**. The answer can differ between packages,
and between an ordinary PR and a PR stacked on an unmerged parent.

## Package anchors retain unfinished release work

The **release branch** is the branch whose merged versions are published.
**Release history** is that branch up to the commit selected for an assessment.

A package's **anchor** supplies the preceding version and source for its
comparison. In actual release history, it is the latest first-parent commit that
changed the package's version. The **anchor version** means exactly the version
declared by that package in the anchor commit.

For example, `widget` reached `1.4.0` in commit `B`:

```text
release history

A ---- B ---- C ---- D
       |           |
       |           +-- unrelated documentation change
       +-- widget 1.4.0
```

The documentation change does not create a new `widget` release. Its anchor is
still `B`. If commit `C` instead changed `widget` without increasing its version,
that work must still be included in the next release. Comparing only with `D`
would hide it.

This is why the tool retains anchors rather than simply comparing every package
with the release branch's latest tree: it can catch up after migration or manual
changes that omitted version planning.

## A stacked PR follows its parent's final result

The **merge target** is the commit a PR intends to merge into. In a stack, that
can be an unmerged parent PR.

Suppose the parent changes `widget` to `1.5.0`, then receives another fix before
review is complete. All of the parent's final content belongs to its `1.5.0`
release. The child must compare with that final snapshot, not the parent's
intermediate version-edit commit.

```text
release history: widget 1.4.0
    |
    +-- parent PR: widget 1.5.0, including all final fixes
            |
            +-- child PR: further changes need a version above 1.5.0
```

For a new or incremented parent package, the parent's final commit acts as the
child's anticipated anchor. Packages still at their release-history version keep
their historical anchors and any catch-up work.

The managed workflow uses squash merges so each PR's final version and content
enter release history in one commit. The anticipated parent then becomes the
actual preceding release. The child does not depend on a merge queue combining
the PRs, and it cannot reuse the parent's version merely because the parent has
not merged yet.

## What the package status means

The report uses the package's anchor version and content to distinguish:

| Status | Meaning |
| --- | --- |
| `needs-increment` | Released content changed, but the version did not advance above the anchor version. |
| `pending-release` | This PR already has a higher version, or introduces the package. |
| `unchanged` | The package still matches its anchor's content and version. |

A pending increment is kept when it covers the full change. If later edits make
the impact stronger, the skill raises it. Merely rerunning the skill is not a
reason for another increment.

Packages with `publish = false` are not publication requests, but their versions
can still participate in [version groups](versions.md#version-groups).

## History movement changes the question

If another PR merges a version that this PR planned to use, that version now
belongs to different content. Likewise, a changed parent can alter what a child
must be compared with. The skill records one history/target pair for the assessment
and refreshes it before applying the plan.

When either input moves, the skill reassesses instead of attaching old results
to the new history. [Recovery](../operations/recovery.md#release-branch-movement)
describes that process.

## Recorded versions do not prove delivery

After `widget 1.5.0` merges, its source can be unchanged against its new anchor
even if its registry upload failed. Package status describes version readiness,
not whether crates.io, a GitHub tag or a binary archive exists.

Publication checks those destinations separately and completes missing work.
That distinction makes a failed upload recoverable without inventing another
version solely to retry it.
