# Analysis

A series is built per `(discriminant set, benchmark identity, metric)`, ordered by git
first-parent topology. The goal is **high signal-to-noise**: report level shifts and trends
that are real and stay silent on measurement jitter. Because
[no engine is deterministic](engines.md#why-no-engine-is-deterministic), the detector treats
every metric as noisy and never trusts a value as exact.

## Two finding methods

Two finding methods are emitted per series and ranked together by descending relative move:

1. **Change-point (step)** — the primary finding. A single most-likely level shift is located
   with the Pettitt nonparametric change-point test, attributing the change to the commit at
   the start of the after-regime. Persistence is built in, so a single-commit blip cannot trip
   it.
2. **Monotonic drift** — a separate finding type for slow trends. A Mann–Kendall trend test
   establishes that a monotonic trend exists and a robust Theil–Sen slope estimates its
   magnitude.

When both fire on one series, the better-fitting model wins, so sharp steps route to the
change-point method and smooth ramps to drift, and the two never double-report one event.

## Noise-aware gating

A candidate change-point is reported only when several gates all hold: a Mann–Whitney rank
test finds the two regimes distinguishable; the move clears a practical-magnitude floor; the
move stands above the series' own residual scatter about the fitted step; and the two regimes
are well-separated populations (an effect-size check). Where points carry confidence intervals,
interval non-overlap is an *additional* veto that can only suppress a candidate, never
manufacture one.

The **practical-magnitude floor** is a single hard threshold below which no finding surfaces,
regardless of engine, direction, or how confidently it was measured — this is what keeps a
low-noise engine like Callgrind from surfacing sub-threshold trivia.

Because a repository has many benchmarks × metrics, every candidate's p-value enters a single
Benjamini–Hochberg false-discovery-rate procedure, and only survivors are reported.

## Analysis modes

The same stored history answers two very different questions, so `analyze` runs in one of two
modes, auto-detected from git topology (there is no flag to force a mode):

| Technique | history | branch |
|---|---|---|
| Change-point (Pettitt + engine gating) | ✅ | — |
| Monotonic drift (Mann–Kendall + Theil–Sen) | ✅ | — |
| Benjamini–Hochberg false-discovery filter | ✅ | — |
| Tip commit vs. base | — | ✅ |
| Improvements reported | opt-in | ✅ |
| Resolved (inactive) findings reported | opt-in | — |

- **history** — the base-branch view: long-range change-point, drift, and false-discovery
  techniques; reports regressions only by default.
- **branch** — the feature-branch view: judges the tip commit's latest state against the
  base, reporting both directions. Only the tip commit lands in the base on merge, so the
  branch's own intermediate history is ignored.

## Re-baselining

History mode distinguishes a change **still in effect** from one **already addressed**, so a
long history does not keep re-flagging handled events:

- **Resolved spikes** — when a level rose and later returned to its prior baseline, the finding
  is inactive: suppressed by default and surfaced only on request.
- **Blessings** — a [blessing](../commands/bless.md) re-baselines a series from the blessed
  commit forward, so the pre-blessing step is no longer re-flagged while earlier points still
  feed the chart.

## Report formats

The three canonical formats — text (canonical, to stdout), Markdown, and JSON — carry the same
data and differ only in presentation; they compose from one pass. JSON is the machine-readable
form: a flat, globally-ranked findings list where each finding is self-describing. A consumer
keys off a top-level "notable" flag and reads each finding's direction, magnitude, and
attribution. Separately, `analyze` can render a condensed Markdown **summary** — a lossy
excerpt for a size-limited downstream consumer.

There is **no severity classification**: a finding's magnitude is conveyed by its
relative-change percent, and which findings warrant action is left to human or agent judgement.
