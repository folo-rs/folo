# Analysis

A series is built per `(discriminant set, benchmark identity, metric)`, ordered by git
first-parent topology. The goal is **high signal-to-noise**: report level shifts and trends
that are real and stay silent on measurement jitter. Because
[no engine is deterministic](engines.md#why-no-engine-is-deterministic), the detector treats
every metric as noisy and never trusts a value as exact.

## Two finding methods

History mode evaluates two finding methods for each series, and the resulting findings are
ranked together by descending relative move:

1. **Change-point (step)** — the primary finding. A single most-likely level shift is located,
   attributing the change to the commit at the start of the after-regime. Persistence is built
   in, so a single-commit blip cannot trip it.
2. **Monotonic drift** — a separate finding type for slow trends, established by a trend test
   and sized by an outlier-resistant slope.

When both fire on one series, the better-fitting model wins, so sharp steps route to the
change-point method and smooth ramps to drift, and the two never double-report one event.

> This page is the mental model. For the mechanism — which test, which threshold, in which order,
> with worked examples — see the [Data pipeline](../appendix/index.md) appendix.

## Noise-aware gating

The gates exist to suppress **noise** — movement the measurement itself manufactures — and
nothing else. They are not a filter for changes you might find uninteresting. A level shift
caused by a new runner, a toolchain bump, or a hardware refresh is a genuine move of the
measured level and **is reported**; deciding that its cause makes it acceptable is your
judgment, recorded with [`bless`](../commands/bless.md). Every floor is therefore tuned to the
measurement noise and to the smallest magnitude worth acting on — never to whether someone
would call a cause acceptable, because a gate wide enough to hide an infrastructure step would
hide the regressions that share its shape.

A candidate change-point is reported only when several gates all hold: the two regimes must be
statistically distinguishable, the move must clear a practical-magnitude floor, it must stand
above the series' own scatter, and the two regimes must genuinely separate rather than merely
differ on average. Where points carry confidence intervals, those act as an *additional* veto
that can only suppress a candidate, never manufacture one. A change-point also needs a minimum
run of points on **each** side, so a series too short to hold two such regimes is not analysed
at all.

The **practical-magnitude floor** has two parts and a move must clear both. A **relative floor**
demands a minimum percentage. An **absolute floor**, in the metric's own units, demands a
minimum magnitude — a handful of instructions is build layout shifting rather than work done, a
fraction of a nanosecond is not worth acting on however confidently it was measured, and a
fraction of an allocation cannot happen. Both apply to **every** metric: without the absolute
floor, a benchmark whose baseline is a couple of nanoseconds turns scheduling jitter into a
double-digit percentage "regression", and without the relative floor a large baseline would flag
on a move that is noise at its scale.

> Each detector applies its own gates in its own order, and the thresholds differ by mode and by
> metric. The [Noise gates](../appendix/gates.md) appendix chapter walks every one of them, with
> the current values and worked examples.

## Judging a branch tip

[Branch mode](#analysis-modes) asks a different question from history mode — not "did this
series change somewhere" but "is one new commit at this level surprising, given how much the
base level moves from commit to commit?" Two properties follow from that:

- **A commit is one observation.** Several stored runs at one commit are re-measurements of a
  single build on a single runner, not independent evidence about the base level, so each
  commit's runs collapse to that commit's median before anything is compared. The comparison
  window is counted in **commits** for the same reason — a run-counted window would shrink to
  a handful of commits wherever a repository records several runs each. History mode does not
  collapse this way: it ranks the series' raw points, so a commit carrying several stored runs
  weighs on it once per run.
- **One new observation, not a second sample.** The tip is judged against a **prediction
  interval** for a single future observation drawn from the recent base commits, so the interval
  accounts for both how much the base level scatters and how well those few commits pin down its
  centre. The move the finding reports is measured from that same centre, so the number you read
  is the number that was tested.

Branch mode also holds its relative floor above history's — a pull-request comment is read by
everyone who touches the branch, so a false alarm costs more there. Where the engine reports
per-point dispersion, further suppression-only vetoes apply.

Whichever test produced a finding also fixes its reported **confidence**, in both modes: the
complement of that test's chance level. Confidence tells you how strong the evidence is, never
which threshold the finding happened to clear.

> The [Detection](../appendix/detection.md) appendix chapter has the full comparison of the two
> modes, including how the base window is narrowed when the base itself recently moved, and why
> the bar for accepting such a boundary is higher than the bar for reporting a finding.

## Controlling false discoveries

A repository has many benchmarks × metrics, so testing each independently would flood the
report. Every candidate enters a single false-discovery-rate procedure, and only survivors are
reported. The family it corrects over is every series this analysis judged — including every
series that produced no candidate — because that is what a false-discovery rate is defined over.
That family is exactly the set of series the report counts as
[judged](#reading-a-silent-report).

One consequence is worth knowing up front: a finding has to clear a stricter bar for a large
judged family than for a small one. The report's judged count is the denominator to inspect.
See [Multiplicity and coverage](../appendix/coverage.md) for why that is the honest trade.

## Reading a silent report

"No notable changes detected" is a claim about the series that were actually **judged**, so
every report states how many that was, in its header:

```text
  runs: 240 (a1b2c3d → e4f5a6b)  in-scope series judged: 12 of 13  regressions: 0
```

A silent report then says exactly what that silence covers, and what it does not:

```text
No notable changes detected among the series that were judged.
  Judged 12 of 13 in-scope series; none moved beyond the measurement floor.
  Not judged: 1 series not measured at the analyzed tip commit; 1 series with too few points
  in the analyzed window.
```

A series is not judged when it was dropped before detection or could not clear the evidence
the gates require:

- **not measured at the analyzed tip commit** — the benchmark is no longer part of the suite
  at the commit being analysed, so a change on it is history, not news.
- **too few points in the analyzed window** — the series is shorter than the minimum the mode's
  detector evaluates.
- **too few points since its blessing** — long enough overall, but its
  [active segment](#re-baselining) is not.
- **not measured on the branch** — branch mode has no tip-side measurement to judge.
- **too few base-branch commits to compare against** — branch mode has a tip measurement but
  too little base history to compare it with.

`Judged 0 of N` is the case to watch: nothing was tested, so the silence is not evidence that
nothing moved. The report says so outright. It usually means history is still accumulating, or
that the benchmarks stopped being collected at the tip commit. When nothing was in scope at all
there is no ratio to print, and the report says instead that nothing it accounted for is
measured at the analyzed commit; with no series reconstructed at all it leads with the fact that
nothing was analyzed.

The denominator is the series that *could* have been judged, which excludes ghosts — and the
verdict above it is decided against the same denominator, so the headline and the ratio cannot
tell you different things. A pull request benchmarks only the packages it impacts while analysis
reads the whole store, so every untouched package leaves a ghost behind; counting those would
leave a healthy run reading as a dozen series judged out of thousands, and train readers to
ignore the one field that exists to stop them ignoring it. Ghosts are still named in the
breakdown a silent report prints, and the [JSON census](#report-formats) counts them, so you can
always ask how much of the store this run did not measure.

Note what the tool does *not* claim either way: it reports that a measured level moved, never
why it moved. Attributing a move — and deciding it is acceptable — is your judgment, recorded
with [`bless`](../commands/bless.md).

## Analysis modes

The same stored history answers two very different questions, so `analyze` runs in one of two
modes, auto-detected from git topology (there is no flag to force a mode):

| Technique | history | branch |
|---|---|---|
| Change-point (Pettitt + engine gating) | ✅ | — |
| Monotonic drift (Mann–Kendall + Theil–Sen) | ✅ | — |
| Tip commit vs. base (Student-t prediction interval) | — | ✅ |
| Benjamini–Hochberg false-discovery filter | ✅ | ✅ |
| Improvements reported | opt-in | ✅ |

- **history** — the base-branch view: long-range change-point and drift detection; reports
  regressions only by default.
- **branch** — the feature-branch view: judges the tip commit's latest state against the
  base, reporting both directions. Only the tip commit lands in the base on merge, so the
  branch's own intermediate history is ignored.

## Comparison-base lag

Branch mode compares the tip against the recent base-side points of the **same** discriminant
set — same project, engine, target triple, and [machine key](../commands/machine-key.md).
Measurements are never compared across machine keys, so on rotating CI pools, where the newest
base commits may have run on a different machine, the branch runner's key can have usable base
data only a few commits behind the merge-base. The comparison quietly reaches back in history.

`analyze` discloses this per affected discriminant set, naming how far behind the comparison
base is and why:

- **discriminant set mismatch** — a newer base run for the benchmark and metric exists, but
  under a different machine key. This is pool rotation, not a gap in coverage.
- **no base data at more recent commits** — no newer base run exists for that series at all.

The warning is advisory metadata: it explains *what the tip was actually compared against* and
never changes which findings are reported or the exit code.

## Re-baselining

A long history should not keep re-flagging an event you have already dealt with. A
[blessing](../commands/bless.md) re-baselines a series from the blessed commit forward, so the
pre-blessing step is no longer re-flagged while earlier points still feed the chart.

Blessing is how you dispose of a shift that is real but not about your code — a runner swap, a
toolchain bump, a deliberate tradeoff. The detector reports that the level moved; you record,
once and against the commit it happened at, that you accept it.

## Report formats

The three canonical formats — text (canonical, to stdout), Markdown, and JSON — carry the same
data and differ only in presentation; they compose from one pass. JSON is the machine-readable
form: a flat, globally-ranked findings list where each finding is self-describing. A consumer
keys off a top-level "notable" flag and reads each finding's direction, magnitude, and
attribution. Separately, `analyze` can render a condensed Markdown **summary** — a lossy
excerpt for a size-limited downstream consumer.

JSON also carries the [coverage accounting](#reading-a-silent-report) as a `census` object, so
automation can require that something was actually judged instead of trusting an empty findings
list:

```json
{
  "census": {
    "total": 14,
    "in_scope": 13,
    "judged": 12,
    "unjudged": 2,
    "coverage": "partial",
    "reasons": [
      { "reason": "ghost", "count": 1 },
      { "reason": "too_few_points", "count": 1 }
    ]
  }
}
```

`coverage` is the verdict's reach as a single stable token — `no_series`, `nothing_in_scope`,
`nothing_judged`, `partial` or `full` — so automation can gate on it without re-deriving the
ratio and reaching a different conclusion than the report it accompanies. Only `full` has no
coverage qualification: every in-scope series was judged, while the verdict still means "no
notable changes detected" for judged series.

`--verbose` goes further, naming each unjudged series individually with the evidence it carried
and the gate rule that declined it.

Human-readable findings include a compact, **topology-accurate** chart: one column per
first-parent commit from the first observation onward, so a commit with no measurement
renders as a gap (a broken line) rather than being collapsed away. Leading gaps are trimmed
and interior gaps kept, and a trailing gap up to the analyzed tip is the visual form of the
"no newer data" disclosure — a benchmark not measured on the most recent commits. History
mode shows the full selected series, including pre-blessing context; branch mode shows the
comparison baseline and a bounded recent tail ending at the tip, dropping the interior branch
commits and drawing the [comparison-base lag](#comparison-base-lag) as the empty columns
between the newest base observation and the tip, so the commit being judged remains visible
without compressing months of history into the same chart.

There is **no severity classification**: a finding's magnitude is conveyed by its
relative-change percent, and which findings warrant action is left to human or agent judgment.
