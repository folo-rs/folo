//! Stable pseudo-random permutations for runtime calibration.

/// FNV-1a offset basis and prime, fixing the seed hash independently of Rust's
/// standard-library hash implementation.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// `SplitMix64` constants from the algorithm's published finalizer.
const SPLITMIX_GAMMA: u64 = 0x9e37_79b9_7f4a_7c15;
const SPLITMIX_MIX_A: u64 = 0xbf58_476d_1ce4_e5b9;
const SPLITMIX_MIX_B: u64 = 0x94d0_49bb_1331_11eb;

/// Seed for one conditional permutation distribution.
///
/// Only sorted values, their rank multiset, and the regime rule enter the hash,
/// so reordering the observed history cannot change which null orderings are
/// sampled. Float values are hashed by their stable bits after signed-zero
/// canonicalization; the ranks retain distinctions between actual tie patterns.
pub(super) fn permutation_seed(values: &[f64], sorted_ranks: &[usize], min_regime: usize) -> u64 {
    let mut hash = b"cbh-change-point-permutation"
        .iter()
        .copied()
        .fold(FNV_OFFSET_BASIS, hash_byte);
    hash = hash_u64(hash, usize_to_u64(min_regime));
    hash = hash_u64(hash, usize_to_u64(sorted_ranks.len()));
    let mut sorted_values = values.to_vec();
    sorted_values.sort_unstable_by(f64::total_cmp);
    for value in sorted_values {
        let canonical = if value == 0.0 { 0.0 } else { value };
        hash = hash_u64(hash, canonical.to_bits());
    }
    for &rank in sorted_ranks {
        hash = hash_u64(hash, usize_to_u64(rank));
    }
    hash
}

fn hash_u64(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash = hash_byte(hash, byte);
    }
    hash
}

fn hash_byte(hash: u64, byte: u8) -> u64 {
    (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
}

fn usize_to_u64(value: usize) -> u64 {
    u64::try_from(value).expect("series lengths and ranks fit u64")
}

/// Stable `SplitMix64` generator used only to choose permutation indices.
pub(super) struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    pub(super) fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(SPLITMIX_GAMMA);
        let mut mixed = self.state;
        mixed = (mixed ^ (mixed >> 30)).wrapping_mul(SPLITMIX_MIX_A);
        mixed = (mixed ^ (mixed >> 27)).wrapping_mul(SPLITMIX_MIX_B);
        mixed ^ (mixed >> 31)
    }
}

/// Fisher-Yates shuffle of `values` in place.
///
/// The modulo index bias is below floating-point resolution for the supported
/// series lengths, while retaining a stable stream across platforms.
#[expect(
    clippy::arithmetic_side_effects,
    reason = "index + 1 is bounded by the series length and therefore cannot overflow"
)]
pub(super) fn shuffle(values: &mut [usize], rng: &mut SplitMix64) {
    for index in (1..values.len()).rev() {
        let bound = usize_to_u64(index + 1);
        let pick =
            usize::try_from(rng.next_u64() % bound).expect("a value below the length fits usize");
        values.swap(index, pick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_depends_on_multiset_not_observed_order() {
        let left_values = [5.0, 2.0, 5.0, 8.0, 2.0];
        let right_values = [8.0, 5.0, 2.0, 2.0, 5.0];
        let ranks = [3, 3, 7, 7, 10];
        assert_eq!(
            permutation_seed(&left_values, &ranks, 2),
            permutation_seed(&right_values, &ranks, 2)
        );
    }

    #[test]
    fn seed_canonicalizes_signed_zero() {
        let ranks = [2, 4];
        assert_eq!(
            permutation_seed(&[-0.0, 1.0], &ranks, 1),
            permutation_seed(&[0.0, 1.0], &ranks, 1)
        );
    }

    #[test]
    fn shuffle_stream_is_reproducible() {
        let mut left = vec![1, 2, 3, 4, 5, 6];
        let mut right = left.clone();
        let mut left_rng = SplitMix64::new(42);
        let mut right_rng = SplitMix64::new(42);
        for _ in 0..20 {
            shuffle(&mut left, &mut left_rng);
            shuffle(&mut right, &mut right_rng);
            assert_eq!(left, right);
        }
    }
}
