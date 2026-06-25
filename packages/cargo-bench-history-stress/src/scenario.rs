//! The synthetic dataset model: the discriminant-set matrix, benchmark
//! identities, and the deterministic value generator that shapes each series.
//!
//! Everything here is a pure function of the [`Scenario`] sizes and seed, so a
//! given seed and size set reproduce a byte-identical dataset.
//!
//! The dataset spans every engine the tool supports. The injected timeline shapes
//! are sized in *relative* terms, so they read the same whichever metric an engine
//! records: deterministic engines (Callgrind instruction counts, `alloc_tracker`
//! allocation counts) store an exact value with no dispersion, so any non-zero
//! step is detected exactly; noisy engines (Criterion wall time, `all_the_time`
//! processor time) store the same shape plus a tight confidence band — kept well
//! below the injected step magnitudes — so the seeded findings still surface while
//! the noise-aware detection path is exercised too.
//!
//! Each benchmark belongs to a *family* (`b % FAMILY_COUNT`) that fixes its
//! timeline shape:
//!
//! * family 0 — a gradual upward drift across the whole history (a `history`-mode
//!   drift finding),
//! * family 1 — a sustained step up at the midpoint (a `history`-mode regression),
//! * family 2 — a sustained step down at ~70% (a `history`-mode improvement),
//! * family 3 — a sustained step up at ~75% that a blessing re-baselines in some
//!   sets but not others (so the same step is suppressed in blessed sets and
//!   flagged elsewhere),
//! * family 4 — stable.
//!
//! Two cross-cutting steps drive the other modes: a single-point step on the
//! latest `main` commit for `b % TIP_DIVISOR == 0` (a `tip`-mode regression that
//! is too short-lived to confirm in `history` mode), and an elevation on the
//! feature branch for `b % BRANCH_DIVISOR == 0` (a `branch`-mode regression).

// The synthetic values are illustrative magnitudes, not real measurements:
// bounded small-integer index arithmetic and lossy integer/f64 conversions here
// cannot misbehave for the sizes used, and exact rounding is irrelevant to the
// shapes being injected.
#![allow(
    clippy::arithmetic_side_effects,
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss,
    clippy::integer_division,
    clippy::modulo_arithmetic,
    reason = "synthetic illustrative values over bounded inputs; lossy/wrapping numeric \
              behavior is intentional and cannot misbehave for the sizes used here"
)]

use cargo_bench_history_core::model::{BenchmarkId, DiscriminantSet, Engine, Metric, MetricKind};
use jiff::Timestamp;
use nonempty::nonempty;

/// Storage project id the seeded config declares and `analyze` partitions under.
pub(crate) const PROJECT: &str = "stress";

/// The synthetic base (integration) branch name.
pub(crate) const BRANCH_MAIN: &str = "main";

/// The synthetic feature branch name.
pub(crate) const BRANCH_FEATURE: &str = "feature";

/// Tool version stamped into every synthetic run and blessing.
pub(crate) const TOOL_VERSION: &str = "stress";

/// Number of benchmark families; a benchmark's family is its index modulo this.
const FAMILY_COUNT: usize = 5;

/// The family whose ~75% step a blessing re-baselines (in the blessed sets).
const BLESSABLE_FAMILY: usize = 3;

/// Discriminant sets at or before this index receive the family-3 blessing; the
/// rest keep the un-blessed step, so the two cases contrast in one run.
const BLESSED_SET_LIMIT: usize = 3;

/// Every benchmark whose index is a multiple of this gets a single-point step on
/// the latest `main` commit — a `tip`-mode regression too short to confirm in
/// `history` mode.
const TIP_DIVISOR: usize = 7;

/// Every benchmark whose index is a multiple of this is elevated on the feature
/// branch — a `branch`-mode regression.
const BRANCH_DIVISOR: usize = 3;

/// Only benchmarks whose index is a multiple of this carry the dirty-snapshot
/// wobble; the rest record dirty snapshots identical to their feature-clean level.
///
/// A deterministic engine flags *any* non-zero move, so a wobble on every
/// benchmark would make every series regress in `branch` mode and drown the
/// intended signal. Scoping the wobble to a sparse subset keeps the dirty
/// snapshots as flavor — exercising the dirty-loading and dirty-tip paths and
/// adding a handful of branch findings — without flooding the result.
const DIRTY_DIVISOR: usize = 11;

/// Smallest synthetic base value (instruction count).
const BASE_MIN: u64 = 1_000;

/// Width of the synthetic base-value range above [`BASE_MIN`].
const BASE_SPAN: u64 = 9_000;

/// Total relative drift accumulated by a family-0 series across the history.
const DRIFT_FRACTION: f64 = 0.20;

/// Relative size of the family-1 midpoint regression step.
const STEP_REGRESSION: f64 = 0.15;

/// Relative size of the family-2 improvement step.
const STEP_IMPROVEMENT: f64 = 0.10;

/// Relative size of the family-3 blessable step.
const BLESSABLE_STEP: f64 = 0.12;

/// Relative size of the single-point tip regression.
const TIP_REGRESSION: f64 = 0.25;

/// Relative size of the feature-branch regression.
const BRANCH_REGRESSION: f64 = 0.20;

/// Relative per-snapshot wobble distinguishing the seeded dirty snapshots.
const DIRTY_WOBBLE: f64 = 0.02;

/// Per-measurement dispersion, as a fraction of the value, stamped onto noisy
/// engines' metrics. Kept far below the injected step magnitudes (10–25%) so the
/// seeded findings still surface on noisy engines while their runs stay
/// well-formed and exercise the noise-aware detection path.
const NOISE_FRACTION: f64 = 0.01;

/// Half-width multiplier (≈ a 95% normal confidence interval) applied to the
/// noisy dispersion band around each point estimate.
const NOISE_CI_Z: f64 = 1.96;

/// The six target triples spanning {windows, linux, macos} × {x64, arm}.
const TRIPLES: [&str; 6] = [
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
];

/// Machine key for the hardware-dependent engines, whose series are partitioned
/// per machine. The synthetic dataset attributes them all to one fixed rig.
const MACHINE_KEY: &str = "stress-rig";

/// Whether `triple` targets Linux — the only platform Callgrind can run on (it
/// drives Valgrind), so Callgrind sets exist for the Linux triples only.
fn targets_linux(triple: &str) -> bool {
    triple.contains("linux")
}

/// The discriminant-set matrix: every supported engine crossed with the target
/// triples it can run on.
///
/// Callgrind is gated to Linux because Valgrind runs there only; the other engines
/// span all six triples. Hardware-dependent engines (Criterion, `all_the_time`)
/// carry a [`MACHINE_KEY`]; the hardware-independent ones (Callgrind,
/// `alloc_tracker`) use the literal `synthetic` machine key. The data is not meant
/// to be realistic across engines — the point is that every engine's partition is
/// populated so `analyze` exercises each one.
pub(crate) fn discriminant_sets() -> Vec<DiscriminantSet> {
    let mut sets = Vec::new();
    for engine in Engine::ALL {
        let machine = engine.is_hardware_dependent().then_some(MACHINE_KEY);
        for triple in TRIPLES {
            if engine == Engine::Callgrind && !targets_linux(triple) {
                continue;
            }
            sets.push(DiscriminantSet::new(engine, triple, machine));
        }
    }
    sets
}

/// The metric a synthetic run records for `engine` at point estimate `value`.
///
/// Deterministic engines record an exact integer count with no dispersion; noisy
/// engines record the estimate plus a tight confidence band, mirroring a real run
/// so the noise-aware gates have the dispersion they need.
pub(crate) fn metric_for(engine: Engine, value: f64) -> Metric {
    let kind = primary_metric(engine);
    if engine.is_hardware_dependent() {
        let std_dev = value * NOISE_FRACTION;
        let half = std_dev * NOISE_CI_Z;
        Metric::new(kind, value).with_dispersion(
            Some(std_dev),
            Some(value - half),
            Some(value + half),
        )
    } else {
        Metric::new(kind, value.round())
    }
}

/// The primary metric kind each engine reports.
fn primary_metric(engine: Engine) -> MetricKind {
    match engine {
        Engine::Callgrind => MetricKind::InstructionCount,
        Engine::AllocTracker => MetricKind::AllocationCount,
        Engine::Criterion => MetricKind::WallTime,
        Engine::AllTheTime => MetricKind::ProcessorTime,
    }
}

/// Whether the discriminant set at `set_index` receives the family-3 blessing.
pub(crate) fn is_blessed_set(set_index: usize) -> bool {
    set_index < BLESSED_SET_LIMIT
}

/// The `bless` prefix that selects the blessable family.
pub(crate) fn blessable_family_prefix() -> String {
    format!("stress/fam{BLESSABLE_FAMILY}/")
}

/// The stable identity of benchmark `index`: `stress/fam{family}/b{index}`.
pub(crate) fn benchmark_id(index: usize) -> BenchmarkId {
    let family = index % FAMILY_COUNT;
    BenchmarkId::new(nonempty![
        "stress".to_owned(),
        format!("fam{family}"),
        format!("b{index:05}"),
    ])
}

/// The sizes and seed that fully determine a synthetic dataset.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Scenario {
    /// Distinct benchmark cases per discriminant set.
    pub(crate) benchmarks: usize,
    /// First-parent commits on the synthetic `main` history.
    pub(crate) commits: usize,
    /// Commits on the synthetic feature branch.
    pub(crate) branch_commits: usize,
    /// Dirty snapshots seeded on the feature tip.
    pub(crate) dirty_runs: usize,
    /// Seed for the deterministic value generator.
    pub(crate) seed: u64,
}

impl Scenario {
    /// The `main` commit index at which the family-3 blessable step appears and
    /// the blessing is recorded (~75% along, ≈ three months before the tip).
    pub(crate) fn bless_index(&self) -> usize {
        3 * self.commits.saturating_sub(1) / 4
    }

    /// Whether `main` commit `index` stores a run.
    ///
    /// Roughly half the commits do — every even index — so the analysis walks a
    /// git topology in which many commits have no stored result, exercising the
    /// "commit with no run" path that a real, sparsely-benchmarked history hits.
    /// The tip and the blessed commit are always populated so `tip`/`branch`
    /// detection and blessing stay well-defined regardless of where the parity
    /// falls.
    pub(crate) fn commit_has_run(&self, index: usize) -> bool {
        let last = self.commits.saturating_sub(1);
        index.is_multiple_of(2) || index == last || index == self.bless_index()
    }

    /// How many `main` commits store a run (the rest are gaps in the history).
    pub(crate) fn commits_with_runs(&self) -> usize {
        (0..self.commits)
            .filter(|&i| self.commit_has_run(i))
            .count()
    }

    /// The deterministic base instruction count for benchmark `b` in set `s`.
    pub(crate) fn base_value(&self, b: usize, s: usize) -> f64 {
        let mix = self.seed
            ^ (b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ (s as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
        let h = splitmix64(mix);
        (BASE_MIN + (h % BASE_SPAN)) as f64
    }

    /// The clean instruction count for benchmark `b` in set `s` at `main` commit
    /// index `i`. `apply_tip` includes the single-point tip step (present only on
    /// the latest commit of the base branch, absent from a branch baseline).
    pub(crate) fn main_clean_value(&self, b: usize, s: usize, i: usize, apply_tip: bool) -> f64 {
        let base = self.base_value(b, s);
        let count = self.commits;
        let position = if count > 1 {
            (i as f64) / ((count - 1) as f64)
        } else {
            0.0
        };
        let mut value = base;
        match b % FAMILY_COUNT {
            // Gradual upward drift across the whole history.
            0 => value += base * DRIFT_FRACTION * position,
            // Sustained step up at the midpoint.
            1 if 2 * i >= count => value += base * STEP_REGRESSION,
            // Sustained step down at ~70%.
            2 if 10 * i >= 7 * count => value -= base * STEP_IMPROVEMENT,
            // Sustained step up at ~75%, possibly blessed away.
            3 if i >= self.bless_index() => value += base * BLESSABLE_STEP,
            _ => {}
        }
        if apply_tip && i + 1 == count && b.is_multiple_of(TIP_DIVISOR) {
            value += base * TIP_REGRESSION;
        }
        value
    }

    /// The clean instruction count for benchmark `b` in set `s` on the feature
    /// branch. The baseline is the family's settled end-of-history level (without
    /// the base branch's tip-only step); `b % BRANCH_DIVISOR == 0` benchmarks are
    /// elevated to introduce a branch regression.
    pub(crate) fn feature_clean_value(&self, b: usize, s: usize) -> f64 {
        let baseline = self.main_clean_value(b, s, self.commits.saturating_sub(1), false);
        if b.is_multiple_of(BRANCH_DIVISOR) {
            baseline + self.base_value(b, s) * BRANCH_REGRESSION
        } else {
            baseline
        }
    }

    /// The instruction count for the `k`-th dirty snapshot on the feature tip.
    ///
    /// Only a sparse subset of benchmarks (see [`DIRTY_DIVISOR`]) wobbles above
    /// the feature-clean level; the rest record a snapshot identical to their
    /// feature-clean value, so the dirty snapshots stay flavor rather than
    /// flagging every series in `branch` mode.
    pub(crate) fn dirty_value(&self, b: usize, s: usize, k: usize) -> f64 {
        let clean = self.feature_clean_value(b, s);
        if b.is_multiple_of(DIRTY_DIVISOR) {
            clean + self.base_value(b, s) * DIRTY_WOBBLE * ((k + 1) as f64)
        } else {
            clean
        }
    }
}

/// `SplitMix64` finalizer: maps a seed word to a well-distributed 64-bit value.
fn splitmix64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Seconds in the rolling 12-month window the synthetic history spans.
const YEAR_SECONDS: i64 = 365 * 24 * 60 * 60;

/// Seconds between successive feature-branch commits (placed just after the
/// anchor, so the branch is the newest history).
const FEATURE_COMMIT_SPACING_SECONDS: i64 = 60 * 60;

/// Committer dates for the synthetic history, oldest first.
///
/// `main` commits are spread evenly across the trailing 12 months ending at
/// `anchor`; feature commits follow just after the anchor. A single `main` commit
/// (`commits == 1`) has no range to spread over and is placed at `anchor` itself.
/// The dates double as each run's `commit` timestamp, which is what `analyze`
/// windows `--since` by.
pub(crate) fn commit_times(
    anchor: Timestamp,
    commits: usize,
    branch_commits: usize,
) -> (Vec<Timestamp>, Vec<Timestamp>) {
    let anchor_s = anchor.as_second();
    let start_s = anchor_s - YEAR_SECONDS;
    let denom = commits.saturating_sub(1).max(1) as i64;
    let main = (0..commits)
        .map(|i| {
            let offset = if commits > 1 {
                YEAR_SECONDS * (i as i64) / denom
            } else {
                YEAR_SECONDS
            };
            at_second(start_s + offset)
        })
        .collect();
    let feature = (0..branch_commits)
        .map(|j| at_second(anchor_s + (j as i64 + 1) * FEATURE_COMMIT_SPACING_SECONDS))
        .collect();
    (main, feature)
}

/// Builds a [`Timestamp`] from a Unix second within the representable range.
fn at_second(second: i64) -> Timestamp {
    Timestamp::from_second(second).expect("synthetic commit second is within the Timestamp range")
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::indexing_slicing,
        reason = "indexing fixed-size synthetic series in test assertions"
    )]

    use super::*;

    /// A modest scenario whose family boundaries land on small commit indices, so
    /// the timeline-shape assertions can address individual commits by hand.
    fn scenario() -> Scenario {
        Scenario {
            benchmarks: 12,
            commits: 4,
            branch_commits: 3,
            dirty_runs: 2,
            seed: 0x1234_5678_9abc_def0,
        }
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-6 * (1.0 + b.abs())
    }

    #[test]
    fn discriminant_sets_cover_every_engine_with_platform_constraints() {
        let sets = discriminant_sets();

        // Callgrind exists for the two Linux triples only; the other engines span
        // all six. Expected: 2 + 6 + 6 + 6 = 20 distinct sets.
        let expected: usize = Engine::ALL
            .iter()
            .map(|engine| {
                TRIPLES
                    .iter()
                    .filter(|t| *engine != Engine::Callgrind || targets_linux(t))
                    .count()
            })
            .sum();
        assert_eq!(sets.len(), expected);
        assert_eq!(sets.len(), 20);

        // Every engine is represented at least once.
        for engine in Engine::ALL {
            assert!(
                sets.iter().any(|set| set.engine == engine.as_str()),
                "missing engine {engine}"
            );
        }

        // Callgrind sets are Linux-only.
        for set in &sets {
            if set.engine == Engine::Callgrind.as_str() {
                assert!(
                    targets_linux(&set.target_triple),
                    "callgrind set on non-Linux triple {}",
                    set.target_triple
                );
            }
        }

        // Hardware-dependent engines carry the machine key; the others are synthetic.
        for set in &sets {
            let engine = Engine::from_name(&set.engine).expect("known engine");
            if engine.is_hardware_dependent() {
                assert_eq!(set.machine_key, MACHINE_KEY, "{}", set.engine);
            } else {
                assert!(set.is_synthetic(), "{}", set.engine);
            }
        }

        // No two sets collide on their storage key.
        let keys: std::collections::HashSet<_> = sets
            .iter()
            .map(|set| set.clean_key(PROJECT, "abc"))
            .collect();
        assert_eq!(keys.len(), sets.len());
    }

    #[test]
    fn benchmark_id_encodes_family_and_index() {
        // fam = index % FAMILY_COUNT; the index is zero-padded to five digits.
        let id = benchmark_id(7);
        let rendered = id.to_string();
        assert!(rendered.contains("fam2"), "{rendered}");
        assert!(rendered.contains("b00007"), "{rendered}");
    }

    #[test]
    fn base_value_is_deterministic_and_seed_sensitive() {
        let s = scenario();
        assert!(close(s.base_value(3, 1), s.base_value(3, 1)));
        // A different seed shifts the base value.
        let other = Scenario {
            seed: s.seed ^ 0xFFFF,
            ..s
        };
        assert!(!close(s.base_value(3, 1), other.base_value(3, 1)));
        // Within the configured band.
        let v = s.base_value(5, 2);
        assert!(
            (BASE_MIN as f64..(BASE_MIN + BASE_SPAN) as f64).contains(&v),
            "{v}"
        );
    }

    #[test]
    fn family_zero_drifts_monotonically_upward() {
        let s = scenario();
        // b = 5 is family 0 (5 % 5) and not a tip benchmark (5 % 7 != 0), so the
        // value rises purely from drift across the whole history.
        let series: Vec<f64> = (0..s.commits)
            .map(|i| s.main_clean_value(5, 0, i, true))
            .collect();
        for pair in series.windows(2) {
            assert!(pair[1] > pair[0], "expected rising drift, got {series:?}");
        }
        let base = s.base_value(5, 0);
        assert!(close(series[0], base));
        assert!(close(series[s.commits - 1], base * (1.0 + DRIFT_FRACTION)));
    }

    #[test]
    fn family_one_steps_up_at_the_midpoint() {
        let s = scenario();
        let base = s.base_value(1, 0);
        // commits = 4 -> step when 2*i >= 4, i.e. i >= 2.
        assert!(close(s.main_clean_value(1, 0, 1, false), base));
        assert!(close(
            s.main_clean_value(1, 0, 2, false),
            base * (1.0 + STEP_REGRESSION)
        ));
    }

    #[test]
    fn family_two_steps_down_late() {
        let s = scenario();
        let base = s.base_value(2, 0);
        // commits = 4 -> step when 10*i >= 28, i.e. i >= 3.
        assert!(close(s.main_clean_value(2, 0, 2, false), base));
        assert!(close(
            s.main_clean_value(2, 0, 3, false),
            base * (1.0 - STEP_IMPROVEMENT)
        ));
    }

    #[test]
    fn family_three_steps_up_at_the_bless_index() {
        let s = scenario();
        let base = s.base_value(3, 0);
        let bless = s.bless_index();
        assert_eq!(bless, 2);
        assert!(close(s.main_clean_value(3, 0, bless - 1, false), base));
        assert!(close(
            s.main_clean_value(3, 0, bless, false),
            base * (1.0 + BLESSABLE_STEP)
        ));
    }

    #[test]
    fn family_four_is_flat() {
        let s = scenario();
        let base = s.base_value(4, 0);
        for i in 0..s.commits {
            assert!(close(s.main_clean_value(4, 0, i, true), base));
        }
    }

    #[test]
    fn tip_step_only_lands_on_the_last_commit_of_tip_benchmarks() {
        let s = scenario();
        // b = 0: family 0 AND a tip benchmark (0 % 7 == 0). The tip step is the
        // gap between the apply_tip and no-tip value on the final commit only.
        let last = s.commits - 1;
        let base = s.base_value(0, 0);
        let with_tip = s.main_clean_value(0, 0, last, true);
        let without_tip = s.main_clean_value(0, 0, last, false);
        assert!(close(with_tip - without_tip, base * TIP_REGRESSION));
        // No tip step before the final commit.
        assert!(close(
            s.main_clean_value(0, 0, last - 1, true),
            s.main_clean_value(0, 0, last - 1, false)
        ));
        // A non-tip benchmark (b = 1, 1 % 7 != 0) sees no tip step at all.
        assert!(close(
            s.main_clean_value(1, 0, last, true),
            s.main_clean_value(1, 0, last, false)
        ));
    }

    #[test]
    fn feature_branch_elevates_only_branch_divisor_benchmarks() {
        let s = scenario();
        // b = 3 is a multiple of BRANCH_DIVISOR (3); b = 1 is not.
        let base3 = s.base_value(3, 0);
        let settled3 = s.main_clean_value(3, 0, s.commits - 1, false);
        assert!(close(
            s.feature_clean_value(3, 0),
            settled3 + base3 * BRANCH_REGRESSION
        ));
        let settled1 = s.main_clean_value(1, 0, s.commits - 1, false);
        assert!(close(s.feature_clean_value(1, 0), settled1));
    }

    #[test]
    fn dirty_wobble_only_affects_the_sparse_subset_and_grows_with_k() {
        let s = scenario();
        // b = 0 is a multiple of DIRTY_DIVISOR (0 % 11 == 0): the snapshots wobble.
        let clean0 = s.feature_clean_value(0, 0);
        let d0 = s.dirty_value(0, 0, 0);
        let d1 = s.dirty_value(0, 0, 1);
        assert!(d0 > clean0);
        assert!(d1 > d0, "later dirty snapshots wobble further");
        // b = 1 is not a multiple of DIRTY_DIVISOR: snapshots equal the clean tip.
        let clean1 = s.feature_clean_value(1, 0);
        assert!(close(s.dirty_value(1, 0, 0), clean1));
        assert!(close(s.dirty_value(1, 0, 1), clean1));

        // Pin the exact wobble formula: clean + base * DIRTY_WOBBLE * (k + 1). This
        // distinguishes the intended `+`/`*` operators from their mutations, which
        // preserve the "grows with k" ordering above but change the magnitude.
        let base0 = s.base_value(0, 0);
        assert!(close(d0, clean0 + base0 * DIRTY_WOBBLE * 1.0));
        assert!(close(d1, clean0 + base0 * DIRTY_WOBBLE * 2.0));
    }

    #[test]
    fn blessed_sets_are_the_low_indices_only() {
        for set in 0..BLESSED_SET_LIMIT {
            assert!(is_blessed_set(set), "set {set} should be blessed");
        }
        assert!(!is_blessed_set(BLESSED_SET_LIMIT));
        assert_eq!(
            blessable_family_prefix(),
            format!("stress/fam{BLESSABLE_FAMILY}/")
        );
    }

    #[test]
    fn commit_times_are_ordered_and_windowed() {
        let anchor = Timestamp::from_second(1_750_000_000).unwrap();
        let (main, feature) = commit_times(anchor, 4, 3);
        assert_eq!(main.len(), 4);
        assert_eq!(feature.len(), 3);

        // The window widths are exact: a 365-day year and a one-hour feature spacing.
        // Asserting the literals (not the symbols) keeps the arithmetic that defines
        // them honest, since the windowing assertions below reuse the same symbols.
        assert_eq!(YEAR_SECONDS, 31_536_000);
        assert_eq!(FEATURE_COMMIT_SPACING_SECONDS, 3_600);

        // main is oldest-first, ends at the anchor, starts a year before it.
        for pair in main.windows(2) {
            assert!(
                pair[1] > pair[0],
                "main commits must be strictly increasing"
            );
        }
        assert_eq!(main[0].as_second(), anchor.as_second() - YEAR_SECONDS);
        assert_eq!(main[main.len() - 1].as_second(), anchor.as_second());

        // feature commits all follow the anchor, evenly spaced and increasing.
        for pair in feature.windows(2) {
            assert!(
                pair[1] > pair[0],
                "feature commits must be strictly increasing"
            );
        }
        assert!(feature[0].as_second() > anchor.as_second());
        assert_eq!(
            feature[1].as_second() - feature[0].as_second(),
            FEATURE_COMMIT_SPACING_SECONDS
        );
    }

    #[test]
    fn single_commit_history_is_well_defined() {
        // The degenerate count == 1 case must not divide by zero.
        let s = Scenario {
            benchmarks: 3,
            commits: 1,
            branch_commits: 1,
            dirty_runs: 1,
            seed: 7,
        };
        let base = s.base_value(0, 0);
        assert!(close(s.main_clean_value(0, 0, 0, false), base));
        let anchor = Timestamp::from_second(1_750_000_000).unwrap();
        let (main, feature) = commit_times(anchor, 1, 1);
        assert_eq!(main.len(), 1);
        assert_eq!(feature.len(), 1);
        // The single main commit sits at the anchor itself (it has no range to
        // spread over), which the `commits > 1` guard selects.
        assert_eq!(main[0].as_second(), anchor.as_second());
    }

    #[test]
    fn roughly_half_the_commits_store_a_run() {
        let s = Scenario {
            benchmarks: 3,
            commits: 10,
            branch_commits: 1,
            dirty_runs: 1,
            seed: 7,
        };
        // commits == 10 -> bless_index 3*9/4 = 6 (even, already a run); the last
        // index 9 is odd and force-populated. So every even index is a run, plus
        // the odd tip: {0,2,4,6,8} ∪ {9}.
        assert_eq!(s.bless_index(), 6);
        for i in 0..s.commits {
            let expected = i.is_multiple_of(2) || i == s.commits - 1;
            assert_eq!(s.commit_has_run(i), expected, "index {i}");
        }
        assert_eq!(s.commits_with_runs(), 6);
        // The tip and the blessed commit are always populated, whatever the parity.
        assert!(s.commit_has_run(s.commits - 1));
        assert!(s.commit_has_run(s.bless_index()));
    }

    #[test]
    fn odd_tip_and_bless_indices_are_forced_populated() {
        // commits == 8 -> last index 7 (odd) and bless_index 3*7/4 = 5 (odd): both
        // would be gaps under pure parity, so the overrides must populate them.
        let s = Scenario {
            benchmarks: 3,
            commits: 8,
            branch_commits: 1,
            dirty_runs: 1,
            seed: 7,
        };
        assert_eq!(s.bless_index(), 5);
        assert!(s.commit_has_run(7), "tip must be populated");
        assert!(s.commit_has_run(5), "blessed commit must be populated");
        // Five even commits plus the two odd overrides.
        assert_eq!(s.commits_with_runs(), 6);
    }

    #[test]
    fn metric_for_matches_engine_determinism() {
        // Deterministic engines store an exact integer count with no dispersion.
        for engine in [Engine::Callgrind, Engine::AllocTracker] {
            let metric = metric_for(engine, 1234.6);
            assert_eq!(metric.kind, primary_metric(engine));
            assert!(close(metric.value, 1235.0));
            assert_eq!(metric.std_dev, None);
            assert_eq!(metric.interval_low, None);
            assert_eq!(metric.interval_high, None);
        }

        // Noisy engines carry a tight band well inside the injected step sizes.
        for engine in [Engine::Criterion, Engine::AllTheTime] {
            let value = 1000.0;
            let metric = metric_for(engine, value);
            assert_eq!(metric.kind, primary_metric(engine));
            assert!(close(metric.value, value));
            let half = value * NOISE_FRACTION * NOISE_CI_Z;
            assert!(close(metric.std_dev.unwrap(), value * NOISE_FRACTION));
            assert!(close(metric.interval_low.unwrap(), value - half));
            assert!(close(metric.interval_high.unwrap(), value + half));
            // The half-width stays far below the smallest injected step (~10%).
            assert!(half < value * 0.05, "noise band too wide: {half}");
        }
    }

    #[test]
    fn splitmix64_matches_reference_outputs() {
        // Pinning the finalizer's exact outputs anchors every bitwise step (the
        // XORs and shifts), which the value-shape assertions elsewhere would
        // otherwise let drift since they only require determinism.
        assert_eq!(splitmix64(0), 16_294_208_416_658_607_535);
        assert_eq!(splitmix64(1), 10_451_216_379_200_822_465);
        assert_eq!(splitmix64(0xDEAD_BEEF_1234_5678), 5_603_369_935_320_146_837);
    }

    #[test]
    #[expect(
        clippy::float_cmp,
        reason = "base_value reduces to BASE_MIN + h % BASE_SPAN, an exact integer-valued f64"
    )]
    fn base_value_matches_reference_outputs() {
        // Exact base values pin the seed/index mixing (the XOR fan-in) and the
        // BASE_MIN + h % BASE_SPAN reduction, so a different bit-mix is detected.
        let s = scenario();
        assert_eq!(s.base_value(0, 0), 5016.0);
        assert_eq!(s.base_value(1, 0), 8212.0);
        assert_eq!(s.base_value(3, 1), 5503.0);
        assert_eq!(s.base_value(5, 2), 6972.0);
        assert_eq!(s.base_value(7, 3), 2697.0);
        assert_eq!(s.base_value(2, 0), 9699.0);
    }

    #[test]
    fn family_one_step_threshold_uses_doubled_index() {
        // commits == 6 separates the real threshold (2*i >= 6, i.e. i >= 3) from a
        // would-be additive one (2 + i >= 6, i.e. i >= 4): only `2 * i` steps at i = 3.
        let s = Scenario {
            benchmarks: 6,
            commits: 6,
            branch_commits: 1,
            dirty_runs: 1,
            seed: 99,
        };
        let base = s.base_value(1, 0);
        assert!(close(s.main_clean_value(1, 0, 2, false), base));
        assert!(close(
            s.main_clean_value(1, 0, 3, false),
            base * (1.0 + STEP_REGRESSION)
        ));
    }
}
