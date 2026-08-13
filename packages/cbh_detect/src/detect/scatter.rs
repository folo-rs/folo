//! Deterministic measurement scatter for tests.
//!
//! What the tests need is *reproducibility*, not statistical excellence. A test whose
//! data differs between runs fails at random and proves nothing, so the scatter is a
//! pure function of a seed and identical on every platform and every invocation.
//!
//! The scatter this produces is bounded, symmetric, and light-tailed, which is enough
//! to keep a "stays quiet" case from being trivially easy but is not the shape real
//! measurement noise takes. Pathological shapes come from recordings instead (see
//! [`recorded`](super::recorded)).

#![cfg_attr(coverage_nightly, coverage(off))]

/// The coefficient of variation a timing series carries.
///
/// Wall-time benchmarks in this project's own stored history run at two to three
/// percent between-commit scatter, and this is the middle of that band. The figure
/// matters because it is what the noise gates are up against in production: a curated
/// series an order of magnitude cleaner makes every "stays quiet" case trivially easy
/// and never reproduces the false positives those gates exist to reject.
pub const TIMING_NOISE_CV: f64 = 0.025;

/// The increment [`NoiseSource`] advances its counter by, from the published
/// `splitmix64` generator.
const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;

/// The first multiplier of the `splitmix64` finalizer.
const SPLITMIX_MIX_A: u64 = 0xbf58_476d_1ce4_e5b9;

/// The second multiplier of the `splitmix64` finalizer.
const SPLITMIX_MIX_B: u64 = 0x94d0_49bb_1331_11eb;

/// A `splitmix64` pseudo-random generator: the source of every synthetic series'
/// measurement scatter.
///
/// `splitmix64` is a handful of arithmetic operations, needs no dependency, and — being
/// counter-based behind a strong finalizer — yields unrelated streams for adjacent
/// seeds, which is what lets a batch of series be independent of one another rather
/// than carrying copies of one sequence.
#[derive(Debug)]
struct NoiseSource {
    /// The counter the finalizer is applied to; the seed is simply its starting value.
    state: u64,
}

impl NoiseSource {
    /// A generator whose stream is determined entirely by `seed`.
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// The next deviate, uniform on `[-1, 1]`.
    fn next_deviate(&mut self) -> f64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        mixed ^= mixed >> 31;
        // The top half is the best-mixed one and lands in a `u32` exactly, which keeps
        // the conversion to `f64` lossless.
        let bits = u32::try_from(mixed >> 32).expect("the top half of a u64 fits a u32");
        f64::from(bits) / f64::from(u32::MAX) * 2.0 - 1.0
    }
}

/// The offset basis of the 64-bit FNV-1a hash, which turns a series name into a seed.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;

/// The prime of the 64-bit FNV-1a hash.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// The scatter seed a series called `name` draws from.
///
/// Keying the seed to the name is what makes every series in a batch independent: each
/// name hashes to a different seed, so a batch is a batch of distinct series rather than
/// one series repeated.
#[must_use]
pub fn seed_of(name: &str) -> u64 {
    name.bytes().fold(FNV_OFFSET_BASIS, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// `values` carrying measurement scatter at coefficient of variation `cv`, drawn from
/// `seed`.
///
/// The deviates are uniform on `[-h, h]`. A uniform deviate's standard deviation is
/// `h/√3`, so the half-width is the requested coefficient of variation scaled by `√3` —
/// which makes `cv` the series' actual coefficient of variation rather than its peak
/// excursion.
///
/// The scatter is relative to each point's own level, so scaling a whole series scales
/// its scatter with it. A `cv` of zero returns the values untouched.
#[must_use]
pub fn scattered(values: &[f64], cv: f64, seed: u64) -> Vec<f64> {
    let half_width = cv * 3.0_f64.sqrt();
    let mut noise = NoiseSource::new(seed);
    values
        .iter()
        .map(|&value| value.mul_add(half_width * noise.next_deviate(), value))
        .collect()
}
