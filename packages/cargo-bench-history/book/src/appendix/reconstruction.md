# Reconstruction

[Selection](selection.md) decided which stored runs are eligible. This stage turns them into
the thing detection actually looks at: **series**.

Four things happen here, and each one changes what detection can see. Three of them *remove*
data, which makes this the stage most often responsible for a report that surprises you.

## Terms used here

| Term | What it means |
|---|---|
| **series** | One metric of one benchmark, tracked across commits. |
| **ghost** | A benchmark that history remembers but the analyzed commit no longer measures. |
| **blessing** | A recorded decision to treat a change as accepted, so history stops reporting it. |

## From runs to series

A stored run is a snapshot: one engine's whole output at one commit. A series is the opposite
cut through the same data — one measurement, across every commit.

A series is identified by three things together:

- the **discriminant set** it was measured under,
- the **benchmark identity**, and
- the **metric kind**.

So a single Callgrind benchmark contributes three series, and a run holding twenty benchmarks
scatters points across sixty of them.

{{#include generated/reconstruction-fold.svg}}

Each resulting point carries its value, its position in the first-parent topology, whether it
came from a dirty working tree, and the confidence interval the engine reported if it reported
one.

Note where the position comes from: **git, read at analysis time** — not from anything stored
in the run. When a measurement was taken has no bearing on where it sits in the series.

## Ordering

Points sort by topological position, then clean before dirty, then by the order the objects
were selected.

The clean-before-dirty rule matters at the analyzed tip. A dirty snapshot is *newer* work than
the clean run at the same commit, so it sorts last and becomes the series' latest state, which
is what branch mode judges.

## Gaps are holes, not zeroes

A commit with no observation produces **no point**. Nothing is interpolated, and nothing is
filled in.

{{#include generated/reconstruction-gap.svg}}

The consequence is the single most important thing to understand about this stage, and it is
easy to miss:

> **The detectors cannot see how far apart two observations are.**

A series is a *sequence* to the statistics. Ten observations spread across a thousand commits
and ten across ten consecutive commits are indistinguishable to every detector and every gate.

Charts, on the other hand, are drawn against topology and *do* show gaps to scale. So the
picture tells you something the statistics never knew — which is deliberate, and why a
suspicious finding is always worth looking at as a chart.

*Why not interpolate?* A gap means nothing was measured, not that nothing changed.
Interpolating would invent data, and inventing data in the input to a change detector
manufactures changes.

## Ghost elimination

A benchmark with **no observation at the analyzed commit** is dropped entirely — every one of
its metric series, before anything else happens.

{{#include generated/reconstruction-ghost.svg}}

Three details matter:

- **Any run at that commit counts as presence**, including one from a dirty working tree.
- **One metric rescues the rest.** If a benchmark's instruction count was recorded at the tip,
  its branch counts survive too.
- **It runs before blessings and before detection**, so a ghost never enters the
  [false-discovery family](coverage.md) and never dilutes the correction.

*Why:* re-reporting a benchmark that no longer exists is noise, and a deleted benchmark's last
few commits look exactly like a regression that was never fixed.

*The cost:* a benchmark measured on ninety-nine of a hundred commits, but not the tip, produces
nothing at all. If a run failed at the tip, the report will be quieter than it should be — and
the census is where that shows up. This is the most common reason for a surprising silence;
see [Insights](insights.md).

Renaming a benchmark does the same thing, from the tool's point of view: the old identity stops
being measured and becomes a ghost, and the new one starts a fresh series with no history.

## Blessings

A [blessing](../commands/bless.md) records that you looked at a change and accepted it. It
re-baselines the series: detection sees only the commits from the blessed one onward.

{{#include generated/reconstruction-blessing.svg}}

The full series is kept for charting, so the chart still shows you the whole story while
detection judges only the active part. A blessed series that is left with too few points to
judge is reported with its own distinct reason, rather than being lumped in with series that
were simply too short.

Two asymmetries worth knowing:

- **History mode only.** Branch mode ignores blessings entirely — see
  [Limits](limits.md#branch-mode-ignores-blessings).
- **Prefixes match as plain string prefixes.** Blessing `foo/bar` also accepts `foo/barbaz`.
  Bear that in mind when benchmark names share a stem.

## Several runs at one commit

Two runs at the same commit are two independent measurements of the same thing, and the two
modes treat them differently:

- **History mode** keeps them as separate points. Both are evidence about that commit's level.
- **Branch mode** collapses them to one level per commit before comparing.

{{#include generated/reconstruction-commit-median.svg}}

*Why the difference:* branch mode's comparison window is measured **in commits**, and its
statistics assume one observation per commit. If several runs at one commit each counted
separately, a commit that happened to be benchmarked five times would dominate the base level,
and the size of the window would depend on how often each commit was run rather than on how
much history it covers.

## What reconstruction hands on

A set of series, each an ordered sequence of points, with ghosts removed and blessed series
sliced to their active window.

Next: [Detection](detection.md).
