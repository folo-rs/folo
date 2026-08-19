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

/// A wall-time series holding one commit where the runner lost time to something else,
/// recorded from this project's own stored results.
///
/// It opens on one level, steps down to another, and then holds that second level for the
/// rest of its history apart from a single commit that measures around half as much again
/// as its neighbours. That commit is not a property of the code: the same run recorded
/// comparably inflated figures across the whole family of multithreaded benchmarks
/// measured alongside it, and the series returns to its established level at the very next
/// commit and stays there.
///
/// This is the workspace's reference case for runner interference. Its value is that no
/// noise model produces it: a generator's deviates are bounded and symmetric, whereas the
/// excursion here is one-sided, several times the series' own spread, and gone as
/// abruptly as it arrived — the shape branch mode has to discard rather than average in.
pub const CONTENDED_RUNNER_EXCURSION: [f64; 32] = [
    1141.117, 1139.266, 1129.181, 1155.445, 1146.464, 1139.776, 1143.118, 1155.482, 1041.326,
    1041.057, 1044.074, 1044.310, 1034.942, 1054.552, 1050.274, 1034.325, 1045.818, 1581.042,
    1045.424, 1051.638, 1040.738, 1047.509, 1044.247, 1043.722, 1040.797, 1044.360, 1049.268,
    1060.142, 1051.753, 1042.930, 1042.876, 1043.306,
];

/// Where the level [`CONTENDED_RUNNER_EXCURSION`] settles on begins: the commit that
/// stepped it down off its opening level.
///
/// A base window taken from here holds one level and one excursion, which is the shape
/// the branch-mode rows exercise. Taken from the recording's start it would straddle the
/// step as well, mixing two questions into one case.
pub const CONTENDED_RUNNER_LEVEL_START: usize = 8;

/// How much of [`CONTENDED_RUNNER_EXCURSION`] a branch-mode case takes as its base side.
///
/// Chosen so that the recent window branch mode reads holds the excursion with several
/// ordinary commits on either side of it. The excursion has to sit clear of both window
/// edges for the case to be about the excursion rather than about the window's endpoint
/// handling.
pub const CONTENDED_RUNNER_BASE: usize = 27;

/// A value [`CONTENDED_RUNNER_EXCURSION`] settles at once past
/// [`CONTENDED_RUNNER_LEVEL_START`] — an entirely ordinary commit for that series, and so
/// the context run that must not be reported as a move.
pub const CONTENDED_RUNNER_LEVEL: f64 = 1045.0;
