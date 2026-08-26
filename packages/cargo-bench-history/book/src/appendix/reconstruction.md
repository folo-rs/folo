# Reconstruction

[Selection](selection.md) decided which stored runs are eligible. This stage turns them into
the thing detection actually looks at: **series**.

Four things happen here, and each one changes what detection can see. Three of them *remove*
data, which makes this the stage most often responsible for a report that surprises you.

## Terms used here

{{#include generated/terms-reconstruction.md}}

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

The two panes are the same series twice. The first is what the **history** holds: each
observation at the commit it was measured at, gaps drawn to scale, so a steady climb reads as the
straight line it is even though most commits were never measured. The second is what the
**detector receives**: a bare sequence, with no notion of a commit it holds no observation for.
The same climb now reads as uneven steps, because the gaps the detector cannot see are exactly
where the biggest jumps happened.

The consequence is the single most important thing to understand about this stage, and it is
easy to miss:

> **The detectors cannot see how far apart two observations are.**

Ten observations spread across a thousand commits and ten across ten consecutive commits are
indistinguishable to every detector and every gate. The report's charts, like the history pane,
are drawn against topology and *do* show gaps to scale — which is why a suspicious finding is
always worth looking at as a chart: it shows you what the statistics never knew.

*Why not interpolate?* A gap means nothing was measured, not that nothing changed.
Interpolating would invent data, and inventing data in the input to a change detector
manufactures changes.

## Ghost elimination

A benchmark with **no observation at the analyzed commit** is dropped entirely — every one of
its metric series, before anything else happens.

{{#include generated/reconstruction-ghost.svg}}

A couple of details matter:

- **Any run at that commit counts as presence**, including one from a dirty working tree.
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

A few details worth knowing:

- **The evidence line depends on the mode.** History applies the blessing to the context history.
  Branch mode applies it to the base ref's first-parent history, because that is the evidence the
  branch is judged against. In both cases the blessed commit remains and earlier evidence is
  excluded.
- **A blessing is a prefix of the benchmark identity.** The blessed value is matched as a plain
  string prefix of the slash-joined benchmark identity (`group/case`, `package/module/fn`, …),
  so blessing `foo/bar` also accepts `foo/barbaz`. Bear that in mind when names share a stem.
- **A blessing is per discriminant set.** It re-baselines only the series in the discriminant
  set it was recorded against, under the same partitioning rules as everything else; a different
  machine key or target triple is untouched.

## One run per commit

A series carries at most one point per commit. A clean run is stored once per commit and never
rewritten, so it cannot contribute two. The only commit that can hold more than one run is one
with **dirty snapshots** — repeated measurements of an uncommitted working tree — and those are
admitted only on the target side (see [Selection](selection.md#dirty-admission)).

History mode admits no dirty runs, so every commit it reconstructs contributes exactly one
point. Branch mode judges only the context commit, and only that commit's **latest** run: its clean
run, or the newest dirty snapshot taken on top of it. An earlier run at the context commit is a
superseded state, not what is being evaluated, so it takes no part. Either way the context is a
single observation, judged against a base that is itself one clean point per commit.

## What reconstruction hands on

A set of series, each an ordered sequence of points, with ghosts removed and blessings positioned
on the mode's evidence line.

Next: [Detection](detection.md), which locates possible moves and corrects for its own search before
handing candidates onward.
