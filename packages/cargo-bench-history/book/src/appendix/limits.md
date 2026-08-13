# Limits

What the pipeline deliberately does not do, why, and what to do instead.

None of these is a bug, and most are consequences of choices made elsewhere in this appendix.
Knowing them is what keeps you from waiting for a report that will never come.

The first few are stated in full in the chapter that owns them; they are listed here so that one
page answers "what will this tool not tell me?".

## Distance between commits is invisible

Detection is **positional**: a series with ten observations spread across a thousand commits and
one with ten across ten consecutive commits are, to the detectors, the same series.

*What to do:* keep collection dense enough that a drift finding's window means what you think it
means, and read the chart, which does draw gaps to scale. See
[Reconstruction](reconstruction.md#gaps-are-holes-not-zeroes).

## A benchmark missing at the tip disappears entirely

Ghost elimination looks at exactly one commit: the analyzed one. A benchmark measured on
ninety-nine of a hundred commits, but not the tip, contributes nothing.

*What to do:* if a run failed at the tip, collect it again rather than reading the report. The
census names ghosts, so the report always tells you this happened. See
[Reconstruction](reconstruction.md#ghost-elimination).

## Findings never fail a build, and are not classified by severity

The exit code reflects whether analysis *ran*. Findings are ranked by size of move and nothing
else.

*What to do:* read the JSON report, and gate on coverage as well as on findings. See
[Reporting](reporting.md#findings-never-fail-the-build).

## Benchmark identity can collide, and the estimator is invisible

Criterion identities carry no package attribution, so two identically-named benchmarks in
different crates share a series; and the stored record does not say whether a value came from the
slope or the mean.

*What to do:* keep benchmark names unique across the workspace. See
[Shape of the data](shape.md#benchmark-identity).

## One change touching fifty benchmarks produces fifty findings

There is no cross-series aggregation. A change that slows down a shared allocator surfaces
once per affected series, independently judged.

The [group-wide correction](coverage.md) has a consequence here that runs against intuition. It
compares each candidate against a bar that **rises with its rank**, so fifty co-occurring
findings are judged against far more generous bars than a single isolated one would be. Many
simultaneous findings are therefore *easier* to report, which means a wall of findings is weaker
per-finding evidence than its length suggests, not stronger.

*Why:* aggregating would require knowing which series share a cause, which is exactly the
attribution the tool refuses to guess at.

*What to do:* read a report with many simultaneous findings as one event rather than fifty, and
look for the common cause before investigating any of them individually. That pattern is itself
diagnostic — see [Insights](insights.md).
## Confidence is not comparable, and not corrected

A finding's confidence comes from whichever test confirmed it. History and branch findings are
confirmed by different tests answering different questions, so their confidences are not on a
common scale. Neither is adjusted by the group-wide correction that runs afterwards.

*What to do:* use confidence as "how poorly does chance explain this", nothing more. The
report ranks by size of move, which is the comparable quantity.

## Branch mode ignores blessings

A blessing re-baselines a series in history mode. Branch mode does not consult them at all.

*Why:* a blessing accepts a change *in the base's history*. Branch mode's question is whether
the tip differs from the base's current level, and a blessing does not change what that level
is.

## Cross-machine comparison is never attempted

Results from different machine keys are never compared, only reported side by side. If your CI
pool rotates machines, some series will have less base data than the commit count suggests.

*What to do:* the tool discloses this as a comparison-base lag warning rather than silently
comparing. A stable `--machine-key` across a genuinely homogeneous pool is the fix; see
[machine-key](../commands/machine-key.md).

## Some stored facts are never read back

The store records more than the analysis uses:

- **`std_dev`**, which Criterion reports, is stored and never consulted. Analysis uses
  confidence intervals for dispersion checks and the series' own scatter for everything else.
- **`machine`** — the full hardware description behind the machine key — is provenance for a
  human, not an input.
- **`best_of`** records how many repetitions a run took, and does not affect analysis.

*Why:* they cost almost nothing to record and are impossible to recover retroactively. A
future version can use them; a missing history cannot be back-filled with facts nobody wrote
down.

## Spike detection stops on long histories

The search for a resolved spike — a level that rose and came back — scans every candidate
plateau, which costs quadratic time. Above a bounded history length the pass is skipped
entirely, with no note in the report.

*What to do:* nothing, usually; resolved spikes are opt-in and inactive by definition. But do
not read their absence from a long history as evidence there were none.


