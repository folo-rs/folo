//! Generates the selection-adjustment calibration table embedded in `cbh_stats`.
//!
//! # What this crate is
//!
//! `cargo-bench-history` reports a change point with a p-value, but the split position was
//! *chosen* by searching every interior position for the most convincing one. A p-value that
//! ignores that search is optimistic: a series with no real step still tends to throw up one
//! striking-looking split somewhere. The honest, selection-adjusted answer is a mathematical
//! constant of the series length `n` — the null distribution, over all `n!` rank orderings, of the
//! p-value the detector reports — and this crate computes it and writes it into a committed table
//! that [`cbh_stats::selection`] reads at run time.
//!
//! It mirrors `cargo-bench-history-figures` exactly: a `write` / `check` / `verify` binary over a
//! single generated Rust source file, with a freshness test so a stale table fails `just test`.
//! See `docs/design.md` (the book's data-pipeline appendix) for the reader-facing account.
//!
//! # Layout
//!
//! * [`permutation`] — the null-distribution kernel: the detector's procedure evaluated on a rank
//!   ordering, plus exact enumeration and Monte Carlo sampling of it.
//! * [`normal`] — the standard-normal p-value, copied bit-for-bit from `cbh_stats` so the table is
//!   faithful.
//! * [`derive`] — the adjusted-level grid, the conservativeness margin, and the row-by-row ladder
//!   construction.
//! * [`render`] — turns a derived table into the committed source file, and the write/check
//!   contract over it.

mod derive;
mod normal;
mod permutation;
mod render;

pub use derive::{Table, derive_table};
pub use render::{TABLE_PATH, check, write};

/// The shortest history the pipeline judges, and the calibration table's first row.
///
/// Mirrors `cbh_detect::MIN_SERIES_POINTS`; the `matches_production_domain` test pins the two
/// equal. Kept as a local constant rather than a dependency so the `write` binary can regenerate
/// the table when `cbh_stats` (and thus `cbh_detect`) does not yet compile.
pub const MIN_SERIES_LEN: usize = 10;

/// The longest history the pipeline analyzes, and the calibration table's last row.
///
/// Mirrors `cbh_detect::MAX_SERIES_POINTS`; the `matches_production_domain` test pins the two
/// equal. The pipeline discards older points beyond this ceiling (§2.2), so no lookup ever asks
/// for a longer series and the table needs no row above it.
pub const MAX_SERIES_LEN: usize = 1000;

/// The shorter side a located split must reach to be a finding.
///
/// Mirrors `cbh_detect::MIN_REGIME`; the `matches_production_domain` test pins the two equal. The
/// table is calibrated at exactly this value, so it is indexed by `n` alone.
pub const MIN_REGIME: usize = 5;

/// Above this length exhaustive enumeration is infeasible, so rows switch to Monte Carlo.
///
/// `11! ≈ 4.0e7` orderings enumerate in seconds; `12!` is an order of magnitude more. Exact rows
/// need no error margin, so keeping the boundary as high as remains cheap widens the certain part
/// of the table.
pub const EXACT_MAX_N: usize = 11;

/// Orderings simulated per Monte Carlo row in the committed table.
///
/// The conservativeness margin (`derive::dkw_margin`) shrinks as this grows, so this sets how deep
/// into the tail the ladder can certify — and thus the largest Benjamini–Hochberg family the
/// correction serves without over-suppressing (§6.2). Raising it tightens the table at a linear
/// cost in generation time; `verify` re-derives at this same value to reproduce the file exactly.
pub const PRODUCTION_SAMPLES: u64 = 2_000_000;

/// The family-wide failure probability the conservativeness margin is certified at.
///
/// The margin guarantees, with probability at least `1 − DKW_ALPHA` over the generation randomness,
/// that no row under-corrects. Once the file is committed it is deterministic; this bounds only the
/// one-time risk that generation produced an optimistic row.
pub const DKW_ALPHA: f64 = 0.01;

/// The adjusted level the ladder grid is anchored on, so the significance gate is never retuned by
/// grid snapping.
///
/// Equal to `cbh_detect::MAX_CHANGE_CHANCE_LEVEL`; the `matches_production_domain` test pins them
/// equal. Because this exact value is a grid point, an adjusted p-value at the gate compares
/// against the same bits the gate uses.
pub const ANCHOR_LEVEL: f64 = 0.05;

/// The multiplicative spacing between adjacent ladder rungs.
///
/// A rung rounds an adjusted p-value up to the next grid level, so this bounds that rounding at ten
/// percent — fine enough that it never crosses a decision boundary once the grid is anchored on the
/// gate (§6.1).
pub const LADDER_RATIO: f64 = 1.1;

/// The smallest adjusted level the grid extends to.
///
/// Benjamini–Hochberg's strictest bar is `q/m` for a family of `m` judged series; a floor here of
/// `1e-4` keeps the ladder meaningful for families up to `m ≈ 1000`, matching the
/// [`MAX_SERIES_LEN`] ceiling on how many recent points — and so, loosely, how many series — a run
/// reasons about. It sits above `cbh_stats`' `MIN_P_VALUE` floor (`1e-15`) and shares its
/// Benjamini–Hochberg rationale rather than contradicting it. Where Monte Carlo cannot certify this
/// deep, the ladder stops short and the deepest outcomes report the floor (§6.2).
pub const LADDER_FLOOR: f64 = 1e-4;

/// The increment of the workspace's committed `splitmix64` generator (`scatter.rs:25`).
pub(crate) const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// The first multiplier of the `splitmix64` finalizer (`scatter.rs:28`).
pub(crate) const SPLITMIX_MIX_A: u64 = 0xbf58_476d_1ce4_e5b9;

/// The second multiplier of the `splitmix64` finalizer (`scatter.rs:31`).
pub(crate) const SPLITMIX_MIX_B: u64 = 0x94d0_49bb_1331_11eb;

/// The offset basis of the 64-bit FNV-1a hash that turns a row label into a seed (`scatter.rs:67`).
pub(crate) const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The prime of the 64-bit FNV-1a hash (`scatter.rs:70`).
pub(crate) const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// A series length or count as `f64`, exact for every value the table spans.
///
/// Every length here is at most [`MAX_SERIES_LEN`] and every count at most `n!`, both far inside
/// `f64`'s exact-integer range, so the conversion loses nothing.
pub(crate) fn count_f64(count: u64) -> f64 {
    debug_assert!(
        count <= (1_u64 << 53),
        "counts stay inside f64's exact-integer range"
    );
    // `u32::try_from` then `f64::from` would cap at ~4.3e9; counts reach `n!`, so go via the
    // exact-integer range of f64 directly with a precision-loss expectation the range justifies.
    #[expect(
        clippy::cast_precision_loss,
        reason = "count <= 2^53, the exact-integer range of f64, asserted above"
    )]
    let value = count as f64;
    value
}
