# Multiplicity and coverage

A gate-passing candidate has cleared every check that looks at it *individually*. One
question remains, and no amount of scrutiny of a single series can answer it:

**Given how many things were tested, should we believe this one?**

This chapter is that question and its consequence — the report's account of what it actually
covered.

## Terms used here

{{#include generated/terms-coverage.md}}

## Test enough things and something will look surprising

Each gate-passing candidate cleared a test at a chance level below some threshold. Read that
threshold literally: it is the rate at which *unchanged* series are expected to produce a
candidate anyway.

A large unchanged store is therefore likely to produce chance candidates even when the gates
are working. As the judged family grows, the expected number of false candidates grows, and so
does the probability of seeing at least one.

So a per-series check is not enough. The tool applies a **group-wide correction**: sort every
candidate by chance level, and require the strongest to clear a strict bar, the next a
slightly looser one, and so on down. What Benjamini-Hochberg controls is the false-discovery
rate *averaged over many analyses* — the expected share of reported findings that are wrong is
held below the target. It is not a promise about any single report: one report can still carry a
higher wrong share than the target, and only across many analyses does the average settle back
to it.

{{#include generated/coverage-staircase.svg}}

{{#include generated/coverage-staircase.md}}

## The family is every series that could be tested

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

## Why improvements are corrected too

In history mode, improvements are not displayed by default. They are, however, still part of
the family the correction divides by, and they still consume a rank.

That is deliberate, and the arithmetic shows why the obvious alternative is worse. Suppose
ten series were judged, and two produced candidates: an improvement with a very low chance
level, and a regression with a middling one.

{{#include generated/coverage-direction-order.md}}

Filtering improvements out *before* the correction would shorten the sorted list, which moves
the regression to an earlier rank and therefore a **stricter** bar. The regression that is
reported today would be suppressed. Correcting first, then filtering the display, is both the
more sensitive order and the honest one: an improvement is a real discovery that was really
tested, and it is only the *display* that omits it.

## What a report judged

The same definition that builds the family also drives the report's coverage line. That is on
purpose: the number the correction divided by and the number the report claims to have
covered cannot drift apart, because they are the same number.

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
