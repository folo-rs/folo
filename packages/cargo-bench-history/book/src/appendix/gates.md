# Noise gates

[Detection](detection.md) produces candidates. This chapter is the sequence of checks that
decides which of them are worth your attention.

Every gate answers the same underlying question in a different way: **is this move larger
than what the measurement itself manufactures?** A benchmark that reports 100 ns will not
report 100 ns again. The gates exist to keep that fact from becoming a stream of false
alarms.

## Terms used here

| Term | What it means |
|---|---|
| **scatter** | How much a series wobbles between commits when nothing has changed. |
| **typical residual** | How far an ordinary point sits from the level or line fitted to the series. |
| **agreement share** | The fraction of before-and-after pairs that agree the level moved. |
| **confidence interval** | A range the engine reports alongside a measurement, saying how precisely it pinned it down. |
| **quantum** | The smallest step a metric can actually take, such as one whole instruction. |

## What the gates are, and are not

The gates weigh **evidence**, never **cause**.

This is worth stating plainly because it is the most common misreading. If a compiler upgrade
makes your benchmark 8% slower, that is a real move and the tool will report it. The gates
will not suppress it for being "not your fault" — they cannot know whose fault it is, and a
tool that guessed would be worse than one that does not try.

Deciding a reported move is acceptable is your call, and [`bless`](../commands/bless.md) is
how you record it. What the gates remove is movement that **is not there**: jitter, sampling
luck, and moves too small to act on.

## The gates, in order

Gates apply in a fixed order and **stop at the first one that declines**. Nothing below a
veto runs.

That matters for reading the figures below: a ladder that stops three rungs in is not an
incomplete figure, it is an accurate one. The gates after the veto were never evaluated,
because there was no longer a candidate to evaluate them against.

{{#include generated/gates-order.md}}

## A candidate that survives

Start with one that passes everything, so the ladder has a complete shape to compare against.

{{#include generated/gates-ladder-pass.svg}}

Each bar shows what the gate computed against what it demanded, scaled so the demand sits at
the same place on every row. The raw figures are printed alongside, because a p-value, a
percentage and a nanosecond count cannot honestly share an axis.

## Gate 1 — enough evidence

Before anything is measured, the candidate must have enough data to measure. Both sides of a
step need enough commits for their medians to mean anything, and a drift needs enough points
for a trend to be distinguishable from a wander.

A series that falls short is not evaluated and is **counted as unjudged**, not silently
dropped — see [Multiplicity and coverage](coverage.md).

## Gate 2 — the move is big enough to matter

Two floors, and a candidate must clear **both**.

**Relative.** A percentage of the baseline. This is the one people expect.

**Absolute.** A floor in the metric's own units, which exists because a percentage means
nothing at small magnitudes. A benchmark running at 3 ns per iteration that moves to 3.2 ns
has moved 6.7% — comfortably past the relative floor — and told you nothing you can act on.

{{#include generated/gates-absolute-floor.svg}}

The absolute floors differ per metric because the reasons differ:

{{#include generated/gates-floors.md}}

This pair is also what makes the tool scale-invariant in the way you would want: the same
proportional move on a benchmark a thousand times larger clears the absolute floor easily,
and is reported.

## Gate 3 — the move stands out from the series' own scatter

This is the primary noise check, and the one that does the most work. It needs nothing from
the engine — no confidence interval, no repeat runs — because it measures the series against
itself.

The tool fits the candidate's own model to the series (a step, or a line), then measures how
far a typical point sits from that model. The move must be several times that distance.

{{#include generated/gates-residual.svg}}

The shaded band is what the series does anyway. A move inside it is indistinguishable from
the series being itself.

> **A note on the multiplier.** The typical residual here is a plain median distance, not a
> figure rescaled to behave like a standard deviation. So "three times the typical residual"
> is *not* "three standard deviations" — for well-behaved data it is a good deal stricter.
> That is deliberate, but it means you should not translate the multiple into a familiar
> sigma figure and reason from that.

This gate is also why a noisy benchmark is a problem worth fixing rather than tolerating: it
widens the band, and the band is what real moves have to clear. See
[Insights](insights.md) for what to do about one.

## Gate 4 — the two sides genuinely separate

A candidate can pass a significance test and still be an artefact. The classic case is a
series that oscillates between two levels: split it anywhere and the two sides differ, and
with enough points the rank comparison will happily call that difference significant.

So the tool asks a second, different question: **of every possible before-and-after pairing,
what share agree the level moved?**

{{#include generated/gates-agreement-separated.svg}}

A genuine step: nearly every pairing agrees.

{{#include generated/gates-agreement-oscillating.svg}}

A series that revisits both levels: a substantial share of pairings contradict the split, and
the candidate is declined — which matches what a human reads off the chart, namely "noisy,
unchanged".

The important property is that this gate **does not weaken as the series grows**. A
significance test gets easier to pass with more data, because more data makes a small
difference detectable. An agreement share does not: it measures how completely the two sides
separate, which more data estimates more precisely rather than inflating.

## Gate 5 — the engine's own precision does not explain it

Where the engine reports a confidence interval, two further checks apply. Both can only ever
**take a finding away** — no interval ever creates one or relaxes another gate.

**Overlap.** If the two regimes' intervals overlap, the engine is telling you it cannot
distinguish these levels.

{{#include generated/gates-interval-overlap.svg}}

**Noise band.** The move must also exceed a multiple of the engine's typical reported
imprecision, independently of how the two levels compare.

Engines that report no interval — Callgrind always, and the operation engines for a
single-span measurement — simply skip these. They are not held to a weaker standard: gate 3
already judged them against their own between-commit scatter, which is the more relevant
quantity anyway.

## The quantum is not a floor

One distinction that catches people, because the two look alike.

The **absolute floor** (gate 2) decides whether a move is worth reporting. The **quantum** is
something else: the smallest scatter a metric can meaningfully express. An instruction count
cannot resolve finer than one instruction, so when a base window happens to repeat the same
integer, its scatter is treated as one unit rather than zero.

Without that, a window of identical counts would have zero scatter, and *any* difference from
it would read as infinitely certain.

Timing metrics get no quantum at all. A stored time is a slope fitted across many iterations
and resolves far below a clock tick, so imposing a floor would cost short benchmarks real
sensitivity. The price is a corner: a timing base window with *exactly* zero scatter yields
no verdict at all. That is silence rather than false certainty, which is the right trade —
but it is silence, and the census reports it no differently from a series that was judged and
found quiet.

## Two gates that fail open

Worth knowing, because they are surprising: if the tool **cannot compute** a gate's input, the
candidate passes rather than being declined.

A sample of a single point contributes no residual, so gate 3 has nothing to compare against.
A sample with no dispersion produces no statistic. In both cases the move is trusted.

The reasoning is that the alternative — declining anything we cannot check — would silently
suppress findings on exactly the sparse series a user is least likely to be watching. But it
does mean a candidate can reach the report with fewer gates actually applied than the ladder
suggests.

## A candidate that is declined

{{#include generated/gates-ladder-declined.svg}}

{{#include generated/gates-ladder-declined.md}}

Reading a ladder like this is the fastest way to answer "why was my regression not reported?"
— and [Insights](insights.md) turns it into a checklist.

## What the gates hand on

The candidates that survived, each still carrying the chance level of the test that confirmed
it. One question remains, and it is not about any individual candidate:
[given how many things were tested, should we believe this one?](coverage.md)
