//! Verbatim recordings of real benchmark series, shared by the tests that judge the
//! analysis against measured data rather than modelled shapes.
//!
//! A recording is reproduced exactly as it was stored. Nothing here is generated,
//! rounded, or resampled, because the value of these fixtures is precisely that they
//! carry dispersion no noise model produces.

#![cfg_attr(coverage_nightly, coverage(off))]

/// A wall-time series that oscillates between roughly 13 and roughly 25 to 29
/// throughout its whole history, recorded from this project's own stored results.
///
/// A human reading its chart answers "noisy, but nothing changed" without hesitation,
/// so every analysis path that inspects it must stay quiet. It is the workspace's
/// reference case for structured jitter: the two levels recur on both sides of any
/// split a change-point search proposes, so median-based gates — whose breakdown point
/// is 50% — are defeated by it and only an effect-size gate rejects it.
pub const STATIONARY_BIMODAL_NOISE: [f64; 40] = [
    13.26, 14.33, 13.14, 24.97, 13.2, 24.97, 13.17, 25.39, 25.54, 13.18, 13.83, 25.45, 25.02, 25.0,
    13.2, 13.22, 13.24, 13.21, 13.15, 24.97, 26.78, 13.24, 28.98, 10.5, 10.53, 26.76, 26.74, 13.58,
    13.54, 28.86, 14.15, 13.5, 26.77, 25.38, 25.0, 13.97, 26.81, 25.54, 13.62, 13.57,
];

/// How many leading levels of [`STATIONARY_BIMODAL_NOISE`] form the base window that
/// branch mode is exercised against.
///
/// This prefix is the sharpest branch-mode probe the recording offers: its trailing
/// commits happen to land on the low level five times running, so a change-point search
/// over the recent window proposes a split there, and accepting that split would leave a
/// comparison sample whose scatter is a fraction of the window's. A tip at the
/// recording's high level would then read as an enormous, certain regression.
pub const STATIONARY_BIMODAL_BASE: usize = 19;

/// A level [`STATIONARY_BIMODAL_NOISE`] reaches on roughly half of its commits — an
/// entirely ordinary value for that series, and so the context run that must not be
/// reported as a move.
pub const STATIONARY_BIMODAL_HIGH: f64 = 24.97;
