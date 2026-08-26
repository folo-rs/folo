# Multiplicity and coverage

A gate-passing result has cleared every check that looks at its series *individually*. History
detection has also corrected for choices made within that series, such as searching across
possible change points. Each mode then asks a different report-wide question:

- **History:** given how many series were tested, does this candidate survive the group-wide
  false-discovery correction?
- **Branch:** among the comparable base-history arrangements, how many showed at least as much
  out-of-range movement as this branch?

The history answer decides whether the candidate is reported. The branch answer is finite
historical context for an excursion that has already been established from the observed
current-base range; it never suppresses that factual excursion.

This chapter explains those questions and their consequence — the report's account of what it
actually covered.

## Terms used here

{{#include generated/terms-coverage.md}}

## History: test enough things and something will look surprising

Each gate-passing candidate cleared a test at a chance level below some threshold. Read that
threshold literally: it is the rate at which *unchanged* series are expected to produce a
candidate anyway.

A large unchanged store is therefore likely to produce chance candidates even when the gates
are working. As the judged family grows, the expected number of false candidates grows, and so
does the probability of seeing at least one.

So a per-series check is not enough. The tool applies a **group-wide correction**: sort every
candidate the mode can report by chance level, and require the strongest to clear a strict bar,
the next a slightly looser one, and so on down. What Benjamini-Hochberg controls is the
false-discovery rate *averaged over many analyses* — the expected share of reported findings
that are wrong is held below the target, because every rejection is displayed. It is not a
promise about any single report: one report can still carry a higher wrong share than the
target, and only across many analyses does the average settle back to it.

{{#include generated/coverage-staircase.svg}}

{{#include generated/coverage-staircase.md}}

## The history family is every series that could be tested

This is the part that carries the weight, and it is easy to get wrong.

The correction divides by the number of hypotheses **tested**, not the number that produced a
candidate. If the tool fed the correction only the candidates that survived the gates, it
would be dividing by a number that already excludes everything that came out quiet — and the
correction would be almost inert.

So the family is **every series the analysis judged**, including the great majority that raised
nothing at all.

One consequence follows immediately, and it is the honest cost of the design: **a finding has
to clear a stricter bar for a large judged family than for a small one.** The same benchmark,
the same regression, the same data may report when this analysis judged a small family and stay
silent when it judged a large one. The report's judged count is the denominator.

{{#include generated/coverage-family-size.svg}}

{{#include generated/coverage-family-size.md}}

That is not a defect to be worked around. It is the cost of controlling the expected
false-discovery proportion across repeated analyses when you are testing thousands of things
at once. The alternative — a fixed per-series bar — is a report where the expected number of
false alarms grows with the size of your suite, which is precisely the failure that makes
people stop reading benchmark reports.

## Why history filters direction first

In history mode, improvements are neither displayed nor corrected. They leave the candidate set
before the correction runs, so only regressions enter the sorted list.

The judged family does not shrink; the denominator remains every series the analysis judged.
Only the directions being corrected change. Suppose ten series were judged, and two produced
candidates: an improvement with a very low chance level, and a regression with a middling one.

{{#include generated/coverage-direction-order.md}}

Filtering first is the stricter order. It shortens the sorted list, which moves the regression
to an earlier rank and therefore a stricter bar. That costs findings: a regression the other
order would have kept can be suppressed.

That extra sensitivity is not legitimate for a regressions-only report. It is borrowed from a
discovery the reader never sees: the improvement helps the regression pass, then disappears from
the display. The full rejected set may still have a false-discovery bound, but the displayed
findings do not.

The order is conservative in another way too. The detectors' chance levels are two-sided, so an
unchanged series has at most half that chance to raise a candidate in one named direction.
The bar the correction sets is therefore met with room to spare, which is why the target is an
upper bound rather than a level the tool aims at.

## Branch: compare the complete report with base history

Branch mode has one context observation per series, so it does not manufacture per-series chance
levels or feed excursions into the history correction. It keeps every excursion that the observed
current-regime range and practical gates support.

To show how remarkable the **complete report** is, it evaluates comparable reference-lane base
commits as historical branch-like turns. Each turn holds out one base commit as the candidate,
uses the remaining base observations plus the real branch value as references, applies the same
range and noise gates, and sums normalized excess across a shared rectangular series family.

The report states how many historical turns tied with or exceeded the branch score. "None of 10
comparable base commits showed as much out-of-range movement as this branch" means exactly what it
says; it is not relabelled as confidence. The factual findings remain visible when no report-wide
family can be formed, so limited history reduces the context available to the reader rather than
silently changing what was observed.

## What a report judged

The same testability decision that builds history's correction family and branch mode's eligible
series drives the report's coverage line. In branch mode, that verdict includes unresolved
current-base regimes, so the analysis and the report cannot disagree about whether a series was
judged.

Every report states it — the coverage line in the text and Markdown output, and the full census
in the JSON, on every `analyze` run. How each format presents it is the
[Reporting](reporting.md) chapter's subject.

{{#include generated/coverage-census.svg}}

A series that was not judged is always accounted for with a reason — never silently skipped.

{{#include generated/coverage-reasons.md}}

## Reading a silent report

"No notable changes" is not the same statement as "nothing regressed". It means the judged
series produced no reportable move, and the coverage line tells you how much of the
accounted-for data was in scope and judged.

{{#include generated/coverage-states.md}}

**Full** is the only silent state with no coverage qualification: every in-scope series was
judged. The verdict remains "no notable changes detected" for those judged series: no
reportable move survived the gates.

This is why automation should gate on coverage rather than on an empty findings list. A run
that judged nothing at all reports no findings, and a naive check reads that as success.

> **Ghosts are counted apart.** A benchmark the analyzed commit no longer measures is
> excluded from the coverage denominator, though it is still counted in the totals and named
> in the breakdown.
>
> The reason is practical. A pull request typically benchmarks only the packages it touches,
> while analysis reads the whole store — so every untouched package leaves a ghost behind.
> Counting them would make a perfectly healthy run read as "12 of 2000 judged", and a ratio
> that alarming on a good run is a ratio people learn to ignore. See
> [Reconstruction](reconstruction.md) for what makes a ghost.

## What this stage hands on

The findings, and the census. Both go to [Reporting](reporting.md) — which, as it turns out,
gives the census the same prominence as the findings themselves, for the reasons above.
