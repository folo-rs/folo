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

Branch analysis is a dedicated all-series path rather than another independent per-series
detector. Per-series preparation remains parallel: it applies the base blessing boundary,
alternates chronologically ordered observed levels between selector and reference lanes, locates
the latest supported regime using only selectors, and evaluates the context value against the
resulting observed range. The branch tip itself is first collapsed to one observation per commit,
preferring the dirty snapshot lane when present, so a commit with repeated measurements does not
depend on storage order. Alternating observations rather than raw topology coordinates preserves
both lanes for sparse histories. Finalization is serial because the report-wide historical
comparison needs a rectangular family of stable series sharing the same reference-lane candidate
commits.

Selector/reference separation prevents a historical candidate from helping choose the boundary it
is later judged against. The boundary search segments the selector lane: each search takes the
strongest split in its segment and then recurses into both sides, keeping every split that clears
the support gates and adopting the latest of them. Recursing into the earlier side is what makes
the search honest, because the strongest split need not be a supported one and a negligible step
must not discard the candidates behind it. All searches share one predeclared selection-adjusted
error budget, sized for the deepest tree the recursion can build. A current regime starts at the
first selector observation known to be after a split, leaving an interleaved reference observation
out when its side is ambiguous. Histories too short for this separation still support the weaker
complete-window range comparison; a strongly separated recent group too short to establish a
regime makes the series explicitly unjudged.

Range judgment retains every observation in the selected regime. A value is a branch excursion
only outside the recorded minimum or maximum, and its magnitude is excess beyond the nearest edge.
No isolated observation is deleted: doing so would strengthen a finding by hiding contrary
evidence.

The historical comparison treats the real branch and each eligible base commit symmetrically. A
base turn holds out one reference-lane commit and adds the real branch value to the remaining
references. Scores sum normalized, gate-surviving range excesses across the rectangular family.
This comparison supplies report-level context and never suppresses an individual factual
excursion. Family selection deduplicates identical candidate sets, considers chronological
minimum-size windows and every pairwise set intersection, then explores a deterministic bounded
set of additional multi-way intersections. Equal member families are deduplicated before their
complete shared candidate intersection is computed. This admits shared candidates that are not
consecutive in any one series without allowing the intersection search to grow combinatorially.
History mode remains the only path that produces calibrated p-values and applies the
Benjamini–Hochberg filter.

History-mode change-point calibration needs the size of the later false-discovery family before
it can choose its analytic acceptance boundary and permutation precision. Detection therefore
begins with a serial testability prepass that builds the census and obtains its judged count.
The same verdict also governs branch mode, including unresolved current-base regimes, so the
census, the detector, and the verbose diagnostics stay aligned. Workers then run each judged
series independently. The expensive statistical work remains inside the existing per-series
worker chunks; only the cheap classification pass is serial.

Permutation-independent magnitude and noise gates run before selection adjustment. The detector
also fits the drift before calibration and calibrates a step only when that model fits at least as
well; a qualified drift remains the fallback if the step then fails significance. Calibration
combines a conservative analytic split-union bound with conditional permutation under fixed
Bonferroni weights. An analytic result that clears the rank-1 family boundary needs no permutation
work. If only one exact split is admissible, its fixed-split score needs no search correction.
Otherwise calibration enumerates either every distinct rank ordering that fits the budget or every
member of a deterministic finite subgroup. Partial enumeration may stop only when its monotone
lower bound proves the analytic component must win or the candidate must fail. This bounds
candidate work without introducing cross-series allocation decisions that would change the
dependence assumptions of Benjamini–Hochberg. The ceiling is a hard bound, not a claim that every
candidate is cheap: an ambiguous maximum-length series may consume the full orbit.

Parallel work is supplied through an executor abstraction, preserving the same deterministic
analysis logic for production execution and synchronous component tests.

Test data comes from two sources with different jobs. A deterministic generator supplies realistic
*spread* for curated shapes, and verbatim recordings of this project's own stored series supply
realistic *shape* — bimodality and one-sided excursions — which no generator here produces. A gate
whose purpose is to survive a pathological shape is pinned against a recording of that shape
rather than against a model of it.
