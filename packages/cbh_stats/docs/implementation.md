# cbh_stats implementation

`cbh_stats` supports the analysis behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns pure statistical kernels used by `cbh_detect`. It implements mathematical
operations without repository, benchmark, storage, or reporting policy. This boundary keeps the
numerical layer deterministic and independently testable while leaving detector composition and
interpretation to the analysis engine.

## Selection adjustment

The `selection` module owns change-point selection correction described in the
[design](../../cargo-bench-history/docs/DESIGN.md), in "Noise-aware gating" and
"Multiple-comparison discipline". Searching every interior split and retaining the strongest
taints the Mann–Whitney p-value at that split. A correction based only on length is insufficient
because benchmark series commonly contain repeated integer values, and different tie patterns
have different null distributions.

`selection_adjusted_change_point` therefore combines an analytic certificate with conditional
permutation over the exact observed rank multiset. Both are valid p-value components and receive
predeclared Bonferroni weights; taking the smaller weighted component is valid without assuming
they are independent. The result is never allowed below the observed tainted score.

The analytic component applies a union bound over every admissible split. At sizes where
Mann–Whitney is exact, the fixed-split p-value bounds its own contribution. At approximate sizes,
the normal score is used only to invert the lower and upper doubled-rank-sum rejection thresholds.
Each threshold's probability is then bounded under sampling without replacement. Doubled ranks are
normalized to `[0, 1]`, and a sample-mean tail at `x` receives the bound
`exp(-k * D(x || mean))`, where `D` is Bernoulli relative entropy. Hoeffding's convex-order
comparison makes the with-replacement Chernoff bound conservative for the finite population. The
implementation also evaluates Serfling's bounded-mean inequality, which tightens Hoeffding with
the sampling-without-replacement fraction, and keeps the smaller valid tail bound. Summing those
fixed-split tails is conservative regardless of Pettitt's selection rule: a selected score at least
as striking as the observation must occur at one of the splits in the union.
[Hoeffding's comparison theorem] and [Serfling's inequality] supply the finite-population bounds.
When an exact scorer has only one admissible split, its fixed-split p-value needs no search
adjustment: requiring Pettitt to select that split only narrows the rejection event.

The scorer caches every property invariant under permutation. Doubled average ranks make Pettitt
prefix sums and exact subset sums integral; the tie correction and total rank sum are computed
once. The analytic split scan accumulates its smallest and largest attainable rank sums as the
smaller-side size grows instead of rescanning the sorted ranks for every size. Exact Mann–Whitney
tail tables are built lazily and jointly for every feasible smaller-side size only if a selected
split needs one. Near-balanced long splits use the same normal scorer as the production primitive.
The final conditional p-value remains meaningful even when that internal score is approximate
because calibration compares like with like over the observed tie pattern.

Permutation calibration uses a complete conditional orbit rather than a sample from all possible
time orderings. If the number of distinct rank orderings fits the order budget, lexicographic
enumeration visits each exactly once. Otherwise a finite subgroup is enumerated completely,
including the identity. Under the no-change null, the observation is exchangeable within either
orbit, so the fraction at least as extreme is an exact randomization p-value by the
[finite-group randomization principle]. Subgroup stabilizers caused by tied values correctly
contribute repeated orderings with their group multiplicity.

The general fallback group is a direct product of symmetric groups over mixed-radix coordinates.
Its factorization maximizes the product of coordinate factorials within the series-length and order
budgets. Short histories whose best Cartesian action touches too few positions instead use
`A6 × S6` over disjoint position sets. Every action is conjugated by a fixed SplitMix64 bijection
and spread across the history to avoid alignment with contiguous regimes. Conjugation changes which
time positions the group connects without changing closure or order. The mixer is construction
logic only: calibration still enumerates the entire resulting group and does not use pseudorandom
samples.

One shared `SplitScorer` evaluates the observed and every permuted ordering: Pettitt first-maximum
location, minimum-regime rejection, and the same exact-or-normal Mann–Whitney score. Rejected
permuted splits contribute the no-evidence score while remaining in the denominator. During
enumeration, `extreme_so_far / final_orbit_order` is a monotone lower bound on the completed
permutation p-value. Calibration may stop only when this proves that the weighted analytic
component must be the combined minimum or that both components meet the caller's rejection
boundary. It never reports a partial-orbit fraction as a p-value. The caller may skip all
permutation work when the weighted analytic component already clears its acceptance boundary.

[Hoeffding's comparison theorem]: https://doi.org/10.1080/01621459.1963.10500830
[finite-group randomization principle]: https://doi.org/10.1214/aoms/1177729436
[Serfling's inequality]: https://doi.org/10.1214/aos/1176342611

## Rank-test significance

The `stats` module's `MannWhitneyU` owns the significance side of the change-point and branch
gates. It ranks the two samples jointly, keeps the U statistics, and — the point of interest
here — decides *how* to turn them into a two-sided p-value based on the joint sample size,
computing that p-value once at construction.

For a split whose *smaller* side keeps the exact permutation count `C(n1+n2, min(n1, n2))` inside
`f64`'s consecutive exact-integer range (at or below `2^53`) it computes the **exact permutation tail**: it
enumerates the size-`min(n1, n2)` subsets of the joint ranking and sums the mass at least as
extreme as the observed split. This is the behavior the design's "Exact significance where
feasible" section promises. The near-balanced splits, whose central subset count overflows, fall
back to the tie- and continuity-corrected **normal approximation**.

The choice is made per split, not per series (`exact_mw_feasible`): the count peaks at the balanced
split, so a long series still earns the exact tail wherever one side is small. That is deliberate —
a lopsided, heavily tied split is exactly where the normal approximation's deep tail can go wrong
by orders of magnitude; enumerating its small side both fixes that and stays cheap. The
enumeration runs over the smaller side so every intermediate subset count also stays exact —
enumerating the larger side would overflow at its half-size subsets even when the reported answer
is small. Because the count is done in `f64`, no big-integer dependency is pulled in and the kernel
stays Miri-safe. History-mode selection adjustment calibrates whichever path this split-level
scorer takes against the same path over the complete conditional permutation group.

The detector's early population-separation gate needs only Mann–Whitney superiority, not
significance. `mann_whitney_superiority` therefore shares the joint-ranking calculation but omits
exact-tail enumeration. Selection calibration computes significance later only for a preferred
step, avoiding duplicate exact work for candidates that a cheaper gate or the drift fit discards.
