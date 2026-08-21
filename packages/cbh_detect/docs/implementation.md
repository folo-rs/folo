# cbh_detect implementation

`cbh_detect` implements the analysis behavior specified by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). Its place in the
application is defined by the
[`cargo-bench-history` implementation guide](../../cargo-bench-history/docs/implementation.md)
and the workspace rules for [implementation documentation](../../../docs/implementation.md).

The crate owns the I/O-free transformation from loaded run data and repository facts into series
and findings. It composes the kernels in `cbh_stats` with analysis-specific grouping, gating, and
ranking policy. Storage loading and history queries remain in `cbh_analyze`, while presentation of
the resulting findings remains in `cbh_render`.

Gating policy is centralized: every threshold a detector turns on lives in one module rather than
at its point of use, so the policy can be reviewed as a whole and each value carries the reasoning
that sets it. The thresholds are fixed constants with no override mechanism, because the shipped
tool exposes none — so tests exercise the exact policy production runs under rather than scenarios
reachable only by retuning a threshold. Where a test needs to know which gate decided an outcome,
the detectors record their gate evaluations to an optional log it can inspect, rather than relaxing
a threshold to make the decision observable.

Evidence selection is separated from evidence judgment. Deciding which base-window levels a branch
comparison may see — discarding a stale prefix when the base itself moved, and discarding an
isolated measurement excursion — happens once, before the comparison itself is judged, so the
judgment gates see one already-chosen sample and the prediction interval's centre and scatter
always come from that same sample. This is a hard constraint rather than a tidiness preference: a robust scale estimator
paired with a non-robust centre was measured to invent regressions on unchanged code.

Selection is nonetheless bounded by the eligibility gate rather than the reverse. That one gate
runs first, so whether a series can be judged at all is settled on its window as recorded, which
keeps the decision in exact correspondence with the public testability projection the census counts
and the false-discovery family is sized from. Narrowing that happens afterwards cannot make the
census untruthful, because the floor was met before anything was discarded. It follows that no
selection step may discard so much that the remainder falls under the minimum regime length; the
removal allowance is set far below that margin.

History-mode change-point calibration needs the size of the later false-discovery family before
it can choose its analytic acceptance boundary and permutation precision. Detection therefore
begins with a serial, metadata-only testability prepass that builds the census and obtains its
judged count. Workers evaluate the same pure predicate to short-circuit unjudged series, then run
each judged series independently. The expensive statistical work remains inside the existing
per-series worker chunks; only the cheap classification pass is serial.

Permutation-independent magnitude and noise gates run before selection adjustment. The detector
also fits the drift before calibration and calibrates a step only when that model fits at least as
well; a qualified drift remains the fallback if the step then fails significance. Calibration
combines a conservative analytic split-union bound with conditional permutation under fixed
Bonferroni weights. An analytic result that clears the rank-1 family boundary needs no shuffles.
Otherwise the permutation calculation uses Besag-Clifford stopping at a predeclared exceedance
count, with a family-scaled maximum held under an absolute per-candidate ceiling. This bounds
strong-candidate work without introducing cross-series allocation decisions that would change the
dependence assumptions of Benjamini–Hochberg. The ceiling is a hard bound, not a claim that every
candidate is cheap: an ambiguous maximum-length series may consume the full budget, while clear
steps and ordinary nulls take the analytic and sequential exits respectively.

Parallel work is supplied through an executor abstraction, preserving the same deterministic
analysis logic for production execution and synchronous component tests.

Test data comes from two sources with different jobs. A deterministic generator supplies realistic
*spread* for curated shapes, and verbatim recordings of this project's own stored series supply
realistic *shape* — bimodality and one-sided excursions — which no generator here produces. A gate
whose purpose is to survive a pathological shape is pinned against a recording of that shape
rather than against a model of it.
