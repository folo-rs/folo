# Selection adjustment

The change-point detector finds where a series stepped by **trying every interior split and
keeping the most convincing one**. That search is the source of a subtle dishonesty, and this
chapter explains how the tool corrects it before any gate or family-wide correction sees the
number.

> **Two unrelated things are called "selection" in this book.** The
> [Selection](selection.md) chapter is about *which stored runs are eligible* for an analysis.
> This chapter is about a statistical effect — the detector *selecting* the most striking split
> out of many — and has nothing to do with run eligibility. It is also distinct from the
> [family-wide correction](coverage.md), which accounts for how many *series* were tested; this
> one accounts for searching within one series.

## In plain terms: compare the winner with fair reruns

Give the detector an unchanged but noisy series and it will still try many places to cut it in
two, then retain whichever cut happened to look most different. The p-value at that winning split
answers "how surprising is this split?" while ignoring that many alternatives lost the contest.
This book calls that intermediate number the **tainted p**: it is useful as a score, but it is not
an honest chance level for the search that produced it.

The correction reruns the same contest after shuffling the observed measurements into other time
orders. If an unchanged series often produces a winner at least as impressive as the real one,
the apparent step is ordinary chance. If almost none of the shuffled histories can match it, the
step has strong evidence. The fraction that match or beat the real winner is the
**selection-adjusted chance level**.

This is like checking the winner of a card game by repeatedly redealing the same deck. The cards
do not change; only their order does. Reusing the actual deck matters because a benchmark series
often repeats the same integer count many times. A hypothetical deck with all distinct cards can
behave very differently and would give the wrong answer for those ties.

## What one shuffled rerun does

The observed series and every shuffled ordering go through exactly the same scorer:

1. Rank the values, assigning tied values their shared average rank.
2. Let Pettitt inspect every interior position and retain the first strongest split.
3. Treat the ordering as no evidence if either resulting regime is shorter than the production
   minimum.
4. Otherwise score the chosen regimes with the production two-sided Mann–Whitney calculation.

A shuffle rejected by the minimum-regime rule stays in the total as a no-evidence result. Removing
such shuffles would ask "how surprising is the observed winner, assuming a shuffle first produced
a usable winner?" That extra condition would make chance look rarer than it is.

The adjusted value is `(1 + matching shuffles) / (1 + all shuffles)`. Adding the observed case to
both parts prevents a finite sample from claiming an impossible zero chance. The result is also
never allowed below the tainted p, because correcting for a search must not make evidence look
stronger.

## Why this is calculated at runtime

The honest null distribution depends on the measurements' **tie pattern**, not only on how many
measurements exist. A series of twelve distinct values, six pairs, and two repeated levels all
have the same length but produce different rank-score distributions when shuffled. One
length-indexed lookup table cannot honestly represent all of them.

Runtime calibration conditions on the exact rank multiset of the series being judged. It therefore
handles integer counters, quantized measurements, and fully distinct timing values through the
same rule without pretending their null distributions are interchangeable.

## How much shuffling is enough

The adjusted chance level later enters
[Benjamini–Hochberg family filtering](coverage.md). Its strictest boundary becomes smaller as the
number of judged series grows, so a fixed sample count would eventually become too coarse to let
even overwhelming evidence through.

The tool first counts every testable series in the family, then samples 600 shuffled orderings per
judged series for each change-point calculation. If the family has `m` judged series, the budget is
therefore `600 × m`. This gives the finite-sample result enough resolution to pass the strictest
family boundary and expects about 30 matching null orderings at that boundary, rather than basing a
borderline decision on one or two accidents.

Most unchanged or weak series do not spend the full budget. Once enough shuffled orderings have
matched the observed score that the final fixed-budget value cannot possibly clear the later
significance gate, the tool safely stops and returns no evidence. Strong findings use the complete
budget because their deep-tail value matters to the family filter.

## Then both history detectors are accounted for

History runs a change-point detector and a drift detector, then reports whichever model fits the
data better. That is another opportunity to choose a lucky-looking result. The tool therefore
doubles each detector's chance level before its significance gate. This is a conservative
correction for choosing between the two detectors, separate from the shuffling that corrects the
change-point's internal split search.

The resulting per-series chance level is what both the significance gate and the family-wide
filter consume. Branch mode makes one predetermined comparison, searches no split, and needs
neither history-mode correction.

## Exact and approximate split scores

Mann–Whitney scoring is exact whenever the smaller regime has few enough points for all possible
group assignments to be counted exactly. This includes lopsided splits in long histories and is
especially important for repeated integer values, where a textbook large-sample approximation can
be badly wrong.

Near-balanced splits of long histories exceed that counting range and use the tie-corrected normal
score. The tool does not trust this approximate intermediate score as an honest p-value on its
own. It applies the same approximation to the observed ordering and every shuffle, so the final
selection-adjusted value comes from how unusual that score is for this series' actual values. The
score only needs to order the observed and shuffled outcomes consistently; the permutation
distribution supplies the honest chance level.

## Why you can trust and reproduce it

The procedure has no fitted constants or external reference data. Its inputs are the series'
doubled average ranks, the production minimum-regime rule, the judged family size, and named
production significance policies.

The pseudo-random sequence is deliberately stable: a fixed FNV-1a hash of sorted canonical value
bits, the sorted doubled-rank multiset, and the regime rule seeds a fixed SplitMix64 stream; a
fixed Fisher–Yates shuffle turns that stream into orderings. Sorting before hashing means the
observed time order cannot choose a friendlier sample. Signed zero is canonicalized before
hashing, while the ranks retain the actual tie pattern. The same input and policy therefore
reproduce the same result across runs and platforms.

Tests compare the shared scorer with the production Pettitt and Mann–Whitney primitives, cover tied
and untied series, pin the random stream's reproducibility, and exercise the end-to-end detector
with the production constants.

## What bounds the work

Analysis keeps at most the most recent 1,000 points of a series. The tool is designed for dozens to
a few hundred points, so that ceiling is generous in ordinary use while bounding every shuffled
scoring pass and the exact-tail cache. Histories below the evidence floor remain unjudged;
histories beyond the cap deliberately lose their oldest points.

## What this stage hands on

One honest chance level per series, in place of the tainted score. It flows into the
[significance gate](gates.md) and then the [family-wide correction](coverage.md), so every later
stage reasons about a number that already accounts for how its change point was chosen.
