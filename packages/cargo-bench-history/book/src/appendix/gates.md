# Noise gates

[Detection](detection.md) produces history candidates whose chance levels already account for the
detector's within-series choices, and branch candidates that lie outside an observed current-base
range. This chapter is the set of checks that decides which candidates are worth your attention.

Every gate answers the same underlying question in a different way: **is this move larger than
what could be explained by measurement process noise?** A benchmark that reports 100 ns will
typically not report exactly 100 ns the next time. The gates exist to keep that fact from
becoming a stream of false alarms — and every one of them is a *filter*: a gate can only ever
remove a candidate, never create one or strengthen another.

## Terms used here

{{#include generated/terms-gates.md}}

## What the gates are, and are not

The gates weigh **evidence**, never **cause**.

This is worth stating plainly because it is the most common misreading. If a compiler upgrade
makes your benchmark 8% slower, that is a real move and the tool will report it. The gates will
not suppress it for being "not your fault" — they cannot know whose fault it is, and a tool that
guessed would be worse than one that does not try.

Deciding a reported move is acceptable is your call, and [`bless`](../commands/bless.md) is how
you record it. What the gates remove is movement that **is not there**: jitter, sampling luck,
and moves too small to act on.

## The questions the gates ask

There are more gates than there are questions, because several detectors ask the same question
at a different point in their own sequence. The questions are the part worth learning:

| Question | Gates that ask it |
|---|---|
| Is there enough data to judge a candidate at all? | `min_series_points`, `min_regime`, `min_base_commits` |
| Did anything actually change? | `split_located`, `non_zero_delta` |
| Could chance alone have produced the history candidate? | `significance` |
| Is the move big enough to matter? | `relative_floor`, `absolute_floor` |
| Is a history move bigger than what this series does anyway? | `residual_noise` |
| Do the two before-and-after sides genuinely separate? | `regime_separation` |
| Does the engine's own precision explain the move? | `interval_disjoint`, `interval_noise_band` |

The diagnostic ladder starts with Detection's own setup and test rows so it can explain the whole
verdict in one place. `selection_adjustment` is one of those rows: it is the within-series
correction described in [Detection](detection.md#making-the-winning-split-a-fair-test), surfaced
here for visibility. It changes the chance level and never declines a candidate by itself.

## Each detector has its own sequence

There is no single gate order. Each detector applies the gates its own question needs, in the
order that makes sense for it. A change point checks inexpensive magnitude and noise evidence
before running its bounded search correction; a branch comparison first establishes that the
context value is outside the observed range, then checks the excess beyond the nearest edge.

A candidate **stops at the first gate that declines it**. Nothing after that runs, which is why
a gate ladder is often short. That is an accurate picture, not a truncated one.

{{#include generated/gates-order.md}}

Two things in those tables are easy to miss:

- **Branch mode uses a higher relative floor than history mode.** A branch comparison rests on a
  single context observation, and a pull request that cries wolf gets ignored, so the bar is raised.
- **The interval checks are not applied uniformly.** Disjointness is checked wherever two regimes
  are compared — a change point, or a context commit against its base. The noise band belongs to
  drift and branch. A detector applies an interval check only where the comparison it makes has a
  meaningful interval to check.

## Gate logic: is the move big enough to matter? (`relative_floor`, `absolute_floor`)

Two floors, and a candidate must clear **both**.

**Relative** — a fraction of the baseline. This is the one people expect.

**Absolute** — a floor in the metric's own units, which exists because a percentage means
nothing at small magnitudes. A benchmark running at 3 ns per iteration that moves to 3.2 ns has
moved by more than 6% — comfortably past the relative floor — and told you nothing you can act
on.

{{#include generated/gates-absolute-floor.svg}}

The floors differ per metric because the reasons differ:

{{#include generated/gates-floors.md}}

This pair is also what makes the tool behave sensibly across scales: the same proportional move
on a benchmark a thousand times larger clears the absolute floor easily, and is reported.

## Gate logic: is it bigger than what this series does anyway? (`residual_noise`)

This is the primary noise check, and the one that does the most work. It needs nothing from the
engine — no confidence interval, no repeat runs — because it measures the series against itself.

For history, the tool fits the candidate's own model to the series (a step for a change point, a
line for a drift), then measures how far a typical point sits from that model. The move must be
several times that distance. Branch mode uses its stricter observed-range rule instead.

{{#include generated/gates-residual.svg}}

The shaded band is what the series does anyway. A move inside it is indistinguishable from the
series being itself.

> **A note on the multiplier.** The typical residual is a plain median distance, not a figure
> rescaled to behave like a standard deviation. So "three times the typical residual" is *not*
> "three standard deviations" — for well-behaved data it is considerably stricter. Do not
> translate the multiple into a familiar sigma figure and reason from that.

This gate is also why a noisy benchmark is worth fixing rather than tolerating: it widens the
band, and the band is what real moves have to clear. See [Insights](insights.md).

## Gate logic: do the two sides genuinely separate? (`regime_separation`)

A candidate can pass a significance test and still be an artifact. The classic case is a series
that oscillates between two levels: split it anywhere and the two sides differ, and with enough
points a significance test will happily call that difference real.

So the tool asks a second, different question: **of every possible before-and-after pairing,
what share agree the level moved?** That pairwise directional frequency is the agreement
share — a direction-aware effect size, not another significance test.

{{#include generated/gates-agreement-separated.svg}}

A genuine step: nearly every pairing agrees.

{{#include generated/gates-agreement-oscillating.svg}}

A series that revisits both levels leaves a substantial share of pairings contradicting the
split, and the candidate is declined — which matches what a human reads off the chart, namely
"noisy, unchanged".

The important property is that this gate **does not weaken as the series grows**. A significance
test gets easier to pass with more data, because more data makes a smaller difference
detectable. An agreement share does not: it measures how completely the two sides separate,
which more data estimates more precisely rather than inflating.

## Gate logic: does the engine's own precision explain the move? (`interval_disjoint`, `interval_noise_band`)

Where the engine reports a confidence interval, further checks apply. Both can only ever **take a
candidate away** — no interval ever creates one or relaxes another gate.

**Disjointness.** If the two regimes' intervals overlap, the engine is telling you it cannot
distinguish these levels.

{{#include generated/gates-interval-overlap.svg}}

**Noise band.** The move must exceed a multiple of the engine's own reported imprecision,
independently of how the two levels compare.

Engines that report no interval — Callgrind always, and `alloc_tracker` or `all_the_time` for a
single-span measurement — simply skip these. History still has its residual gate; branch mode
still requires an excursion beyond every value actually observed in the current regime.

## Two gates that fail open

Worth knowing, because it is surprising: if the tool **cannot compute** a gate's input, the
candidate passes rather than being declined.

A sample of a single point contributes no residual, so the residual check has nothing to compare
against. A sample with no dispersion produces no interval statistic. In both cases the move is
trusted.

The reasoning is that the alternative — declining anything we cannot check — would silently
suppress findings on exactly the sparse series a user is least likely to be watching. But it
does mean a candidate can reach the report with fewer gates actually applied than its ladder
suggests.

## Reading a gate ladder

Each detector's gates form a **ladder**: the candidate meets them in order and stops at the first
that declines it. The tables below show one row per gate, in the order the detector applies them.
The demand is what the gate required, the computed value is what the candidate supplied, and the
verdict states the outcome. A numeric gate passes when the computed value clears the demand. A
boolean gate must simply hold. Once a gate declines, the rows below it show `not run`.

History compares the step and drift models after their cheap gates. If drift fits better, the step
ladder can end with selection adjustment and significance marked `not run` even though no earlier
step gate declined: the detector chose the better-fitting drift before paying for step calibration.

A candidate that clears every gate:

{{#include generated/gates-ladder-pass.md}}

And one that is declined:

{{#include generated/gates-ladder-declined.md}}

Reading a declined ladder is the fastest way to answer "why was my regression not reported?" —
and [Insights](insights.md) turns it into a checklist.

## What the gates hand on

The candidates that survived. History values carry selection-adjusted chance levels; branch values
carry the observed range and excess beyond its nearest edge. One question remains, and it is not
about any individual candidate:
[given how many things were tested, should we believe this one?](coverage.md)
