# Limits

What the pipeline deliberately does not do, why, and what to do instead.

Some are deliberate trade-offs and others are limits of the current statistical model. Knowing
them is what keeps you from reading more into a report than its evidence supports.

The first few are stated in full in the chapter that owns them; they are listed here so that one
page answers "what will this tool not tell me?".

## Very long histories use the newest 1,000 points

Detection is designed for histories from dozens to a few hundred measurements. When a series grows
beyond 1,000 points, only its newest 1,000 influence the verdict. The older measurements remain
stored and visible in charts; they simply stop pulling the detector toward performance levels that
are no longer current.

*What to do:* use [`examine`](../commands/examine.md) when you need the full recorded shape. To
judge an older era rather than the current one, move the analysis context to a commit in that era.

## A short series cannot report a lone finding in a large family

The detection minimum is the shortest history that is judged. That series still counts toward
the false-discovery family, but a lone perfect step at that length cannot clear the group-wide
bar once the family grows past a handful of series.

*Why:* a rank comparison on so few points has a floor, and the rank-1 bar shrinks with the
family. Those two numbers miss each other well before a production-scale suite.

*What to do:* collect more history on the series you care about. The coverage chapter shows
how family reach grows with length. See
[Multiplicity and coverage](coverage.md#the-detection-minimum-is-not-a-reporting-guarantee).

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

## Analysis does not fail because it found something

The exit code reflects whether the analysis **ran**, not what it found: a report full of
regressions still exits successfully. Whether a finding should fail a build is a decision for the
automation that reads the report — nothing about a finding makes it fail one on its own, and the
tool does not signal analysis failure merely because findings exist. Findings carry no severity
classification either; they are ranked by size of move and nothing else.

*What to do:* read the JSON report and decide in your own automation, gating on coverage as well
as on findings. See [Reporting](reporting.md#findings-never-fail-the-build).

## Benchmark identity can collide, and the estimator is invisible

Criterion identities carry no package attribution, so two identically-named benchmarks in
different crates share a series; and the stored record does not say whether a value came from the
slope or the mean.

*What to do:* keep benchmark names unique across the workspace. See
[Shape of the data](shape.md#benchmark-identity).

## One shared change can produce many findings

There is no cross-series aggregation. A change that slows down shared code or shared runtime
behavior surfaces once per affected series, independently judged.

The [group-wide correction](coverage.md) has a consequence here that runs against intuition. It
compares each candidate against a bar that **rises with its rank**, so co-occurring findings are
judged against more generous bars than a single isolated one would be. Many
simultaneous findings are therefore *easier* to report, which means a wall of findings is weaker
per-finding evidence than its length suggests, not stronger.

*Why:* aggregating would require knowing which series share a cause, which is exactly the
attribution the tool refuses to guess at.

*What to do:* read a report with many simultaneous findings as one event rather than a count, and
look for the common cause before investigating any of them individually. That pattern is itself
diagnostic — see [Insights](insights.md).

## No certainty score is reported

The report gives no confidence or certainty score. History detection reaches its findings through
chance levels; branch detection reaches its findings by comparing the context run against an
observed range, which produces no chance level at all. The two are not measurements of the same
thing, so there is no scale both could be placed on. The group-wide correction history applies
afterwards does not turn its chance levels into a comparable finding score either.

*What to do:* rank findings by size of move, which is the comparable quantity the report provides.

## Branch historical comparison is finite context, not probability

Branch mode reports an excursion when the context value lies outside the observed current-base
range. Its report-wide historical comparison then counts comparable base commits whose complete
report score tied with or exceeded the branch score.

That count is limited by the base evidence available after windowing, regime selection, and
blessings. It does not estimate all possible future benchmark outcomes and is not reported as a
p-value or confidence. With too few shared reference-lane commits, no report-wide comparison is
shown; factual per-series excursions remain.

*What to do:* read the count literally and inspect the chart or
[`examine`](../commands/examine.md) when the base range appears to contain more than one operating
condition. Gather more ordinary base measurements when you need a broader empirical comparison.

## A recent blessing can leave branch evidence short

A blessing is an intentional evidence boundary in both modes. Branch mode keeps the blessed base
commit and excludes every earlier one, even when that leaves too few base commits to judge the
branch.

*Why:* using evidence the blessing explicitly retired would silently undo the user's decision.
The report names the shortfall rather than falling back across the boundary.

## Cross-machine comparison is never attempted

Results from different machine keys are never compared, only reported side by side. If your CI
pool rotates machines, some series will have less base data than the commit count suggests.

*What to do:* the tool discloses this as a comparison-base lag warning rather than silently
comparing. Run `backfill` on each machine so every real hardware partition accumulates a usable
history.

## Some stored facts are never read back

The store records more than the analysis uses:

- **`std_dev`**, which Criterion reports, is stored and never consulted. Analysis uses
  confidence intervals for dispersion checks, history residuals for history noise, and observed
  ranges for branch excursions.
- **`machine`** — the full hardware description behind the machine key — is provenance for a
  human, not an input.
- **`best_of`** records how many repetitions a run took, and does not affect analysis.

*Why:* they cost almost nothing to record and are impossible to recover retroactively. A
future version can use them; a missing history cannot be back-filled with facts nobody wrote
down.
