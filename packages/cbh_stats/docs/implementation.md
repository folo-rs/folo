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

The `selection` module owns the run-time half of the change-point selection correction described
in the [design](../../cargo-bench-history/docs/DESIGN.md) (§8.2, §8.3). The detector locates a
change point by searching every interior split and keeping the most convincing one, so the
Mann–Whitney p-value at the chosen split — the *tainted p* — is optimistic. `change_point_adjusted_p`
turns a tainted p and a series length into an honest, selection-adjusted p-value that obeys the
p-value contract `P(adjusted ≤ a) ≤ a` under the null.

The correction combines two independently valid upper bounds on the honest adjusted value and
keeps the smaller — which, since each bound is at least the truth, is itself at least the truth
and so still a valid p-value. The first is a committed calibration table (`selection/table.rs`)
indexed by series length: the primitive reads the row for the series it is judging and looks up
the rung covering the tainted p. The second is a union bound, `searched_positions × tainted_p`,
Bonferroni over the interior splits the detector could have reported. At run time the primitive
does only that lookup and one multiplication — no permutation, no sampling — so it stays Miri-safe
and allocation-free on the detector's hot path. The table's rows run from `MIN_SERIES_LEN` to
`MAX_SERIES_LEN` with no gaps, mirroring the pipeline's series-length bounds, so every legitimate
lookup is an exact row index; a length outside that range, or a zero split count, is a caller bug
and panics.

Both bounds are needed because each covers where the other is weak. The calibration table is
derived by Monte Carlo, so it cannot resolve adjusted values below its sampling margin (~1e-3):
every row bottoms out at that floor however strong the evidence. Handed straight to the
downstream family correction, that floor would silently discard obvious regressions in any batch
of more than a couple of dozen series. The union bound has no such floor — it scales straight
down with the tainted p — so it carries the deep tail the table cannot reach, while the table
stays tighter near the decision boundary, where borderline findings are won or lost.

The table itself is **generated and certified offline** by the
[`cargo-bench-history-calibration`](../../cargo-bench-history-calibration/) crate, not by this one.
That crate derives the null distribution of the detector's whole procedure per length, builds the
critical-value ladder, and writes `selection/table.rs`; a freshness test fails the build if the
committed bytes drift from a re-derivation. This split keeps the heavyweight, multi-million-sample
derivation out of `cbh_stats`' dependency and test surface while leaving the shipped numbers under
version control and reviewable. The method and the justification of every number live in that
crate's module documentation and in the book's "Selection adjustment" appendix; this crate only consumes the
result.

## Rank-test significance

The `stats` module's `MannWhitneyU` owns the significance side of the change-point and branch
gates. It ranks the two samples jointly, keeps the U statistics, and — the point of interest
here — decides *how* to turn them into a two-sided p-value based on the joint sample size,
computing that p-value once at construction.

For a split whose *smaller* side keeps the exact permutation count `C(n1+n2, min(n1, n2))` inside
`f64`'s exact-integer range (below `2^53`) it computes the **exact permutation tail**: it
enumerates the size-`min(n1, n2)` subsets of the joint ranking and sums the mass at least as
extreme as the observed split. This is the behavior the design's "Exact significance for short
series" section promises, and it is what the selection-adjustment calibration models. The
near-balanced splits, whose central subset count overflows, fall back to the tie- and
continuity-corrected **normal approximation**.

The choice is made per split, not per series (`exact_mw_feasible`): the count peaks at the balanced
split, so a long series still earns the exact tail wherever one side is small. That is deliberate —
a lopsided, heavily tied split is exactly where the normal approximation's deep tail goes wrong,
understating a fully separated repeated-value split's p-value by orders of magnitude; enumerating
its small side both fixes that and stays cheap. The near-balanced splits kept on the approximation
cannot misreport a verdict, because there the smallest honest p-value already sits below the
reporting clamp. The enumeration runs over the smaller side so every intermediate subset count also
stays exact — enumerating the larger side would overflow at its half-size subsets even when the
reported answer is small. Because the count is done in `f64`, no big-integer dependency is pulled
in and the kernel stays Miri-safe.
