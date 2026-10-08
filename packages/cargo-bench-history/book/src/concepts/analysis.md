# Analysis

A series is built per `(discriminant set, benchmark identity, metric)`, ordered by git
first-parent topology. The goal is **high signal-to-noise**: report level shifts and trends
that are real and stay silent on measurement jitter. Because
[no engine is deterministic](engines.md#why-no-engine-is-deterministic), the detector treats
every metric as noisy and never trusts a value as exact.

## History finding methods

History mode evaluates two finding methods for each series, and the resulting findings are
ranked together by descending relative move:

1. **Change-point (step)** — the primary finding. A single most-likely level shift is located,
   and the report names a commit somewhere near that split. Persistence is built
   in, so a single-commit blip cannot trip it.
2. **Monotonic drift** — a separate finding type for slow trends, established by a trend test
   and sized by an outlier-resistant slope.

When both fire on one series, the better-fitting model wins, so sharp steps route to the
change-point method and smooth ramps to drift, and the two never double-report one event.

> This page is the mental model. For the mechanism — which test, which threshold, in which order,
> with worked examples — see [Data pipeline](../appendix/index.md).

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
> metric. The [Noise gates](../appendix/gates.md) chapter walks every one of them, with
> the current values and worked examples.

## Branch excursion method

[Branch mode](#analysis-modes) asks a different question from history mode — not "did this
series change somewhere" but "is this context commit outside every value recently observed in
the base ref's current regime?" Two properties follow from that:

- **A commit is one observation.** Several stored runs at one commit are re-measurements of a
  single build on a single runner, not independent evidence about the base level, so each
  commit's runs collapse to that commit's median before anything is compared. The comparison
  window is counted in **commits** for the same reason — a run-counted window would shrink to
  a handful of commits wherever a repository records several runs each. History mode does not
  collapse this way: it ranks the series' raw points, so a commit carrying several stored runs
  weighs on it once per run.
- **One new observation, not a second sample.** The context commit is judged against the
  **observed range** of the current base regime. It reports only when the value is strictly below
  every observed base value or strictly above every one, and reports the excess beyond the nearest
  range edge. It does not invent a probability distribution for the one branch observation.

Branch mode also holds its relative floor above history's — a pull-request comment is read by
everyone who touches the branch, so a false alarm costs more there. Where the engine reports
per-point dispersion, further suppression-only vetoes apply.

Neither mode attaches a confidence score to a finding. History reports have calibrated chance
levels internally; branch reports instead state how many comparable base commits produced at least
as much report-wide out-of-range movement. Reports rank individual findings by the size of the
move.

> The [Detection](../appendix/detection.md) chapter has the full comparison of the two
> modes, including how selector and reference observations prevent the base commit being judged
> from choosing its own regime boundary.

## Controlling false discoveries

A repository has many benchmarks × metrics, so a per-series result is not enough to understand a
whole report. History candidates enter a false-discovery-rate procedure whose family is every
history series this analysis judged, including series that produced no candidate.

One consequence is worth knowing up front: a history finding has to clear a stricter bar for a large
judged family than for a small one. The report's judged count is the denominator to inspect.
Branch mode cannot honestly apply that procedure to one observation. It keeps factual excursions
and compares the complete branch report with the same analysis run on eligible base commits in
turn. See [Multiplicity and coverage](../appendix/coverage.md) for both controls.

Configured [ignore prefixes](../commands/analyze.md#ignoring-benchmarks) remove benchmarks
from analysis without discarding their measurements. Ignored series and ghosts sit outside
the in-scope coverage denominator, but remain disclosed in the report's account. If those
exclusions leave nothing in scope, the outcome is `nothing_in_scope`, not an all-clear.

For practical interpretation, see
[Reading a silent report](../appendix/insights.md#reading-a-silent-report).

## Analysis modes

The same stored history answers two very different questions, so `analyze` runs in one of two
modes, auto-detected from git topology (there is no flag to force a mode):

| Technique | history | branch |
|---|---|---|
| Change-point (Pettitt + engine gating) | ✅ | — |
| Monotonic drift (Mann–Kendall + Theil–Sen) | ✅ | — |
| Context commit vs. current-base observed range | — | ✅ |
| Benjamini–Hochberg false-discovery filter | ✅ | — |
| Symmetric historical report comparison | — | ✅ |
| Improvements reported | — | ✅ |

- **history** — the base-branch view: long-range change-point and drift detection; reports
  regressions only.
- **branch** — the feature-branch view: judges the context commit's latest state against the
  base ref, reporting both directions. The branch's own intermediate history is ignored.

## Comparison-base lag

Branch mode compares the context commit against the recent base-ref points for the **same
project** and the **same** discriminant set — same engine, target triple, and machine key.
Measurements are never compared across machine keys, so on rotating CI pools, where the newest
base-ref commits may have run on a different machine, the context run's key can have usable base
data only a few commits behind the base ref. The comparison quietly reaches back in history.

`analyze` discloses this per affected discriminant set, naming how far behind the comparison
base is and why:

- **discriminant set mismatch** — a newer base-ref run for the benchmark and metric exists, but
  under a different machine key. This is pool rotation, not a gap in coverage.
- **no base data at more recent commits** — no newer base-ref run exists for that series at all.

The warning is advisory metadata: it explains *what the context commit was actually compared
against* and never changes which findings are reported or the exit code.

## Re-baselining

A long history should not keep re-flagging an event you have already dealt with. A
[blessing](../commands/bless.md) re-baselines a history series from the blessed commit forward, so
the pre-blessing step is no longer re-flagged while earlier points still feed the chart. In branch
mode it is a hard evidence boundary: the blessed base commit remains, but every earlier base
observation is excluded from regime selection, range construction, and historical comparison.

Blessing is how you dispose of a shift that is real but not about your code — a runner swap, a
toolchain bump, a deliberate tradeoff. The detector reports that the level moved; you record,
once and against the commit it happened at, that you accept it.

## Report formats

The canonical formats — text (to stdout), full Markdown, and JSON — carry the same
findings and compose from one pass. JSON is the machine-readable
form: a flat, globally-ranked findings list where each finding is self-describing. A consumer
keys off a top-level "notable" flag and reads each finding's direction, magnitude, and
attribution. Separately, `analyze` can render a condensed Markdown **summary** — a lossy
excerpt for a size-limited downstream consumer.

Full Markdown ends with a **Coverage** section listing counts and reasons for unjudged metric
series, whether or not findings exist. JSON carries the same account under `census`.
Text and condensed summaries give the judged ratio in their headers and the reason breakdown
only when there are no findings. [Insights](../appendix/insights.md#reading-a-silent-report)
shows matching Markdown and JSON examples and explains how to interpret incomplete coverage.

Human-readable findings include a compact, **topology-accurate** chart: one column per
first-parent commit from the first observation onward, so a commit with no measurement
renders as a gap (a broken line) rather than being collapsed away. Leading gaps are trimmed
and interior gaps kept, and a trailing gap up to the analyzed context commit is the visual form of
the "no newer data" disclosure — a benchmark not measured on the most recent commits. History
mode shows the full selected series, including pre-blessing context; branch mode shows the
comparison baseline and a bounded recent tail ending at the context commit, dropping the interior
branch commits and drawing the [comparison-base lag](#comparison-base-lag) as the empty columns
between the newest base observation and the context commit, so the commit being judged remains
visible without compressing months of history into the same chart.

There is **no severity classification**: a finding's magnitude is conveyed by its
relative-change percent, and which findings warrant action is left to human or agent judgment.
