//! Machine fingerprint: a stable hash of the host's hardware characteristics,
//! used to partition hardware-dependent (Criterion) results so that only runs
//! from equivalent machines share a series.
//!
//! The fingerprint must be reproducible across tool versions and across the
//! equivalent machines of a single pool, so it is a fixed SHA-256 over a
//! versioned, canonical string of hardware factors — never a seeded or
//! process-specific hash. Cloud machines in one pool share these factors and so
//! share a key; genuinely different hardware produces a different key. The host
//! *name* is deliberately not a factor, since pool machines have distinct names
//! but equivalent hardware.
//!
//! The key is surfaced two ways for operators. [`resolve_machine_key`] yields the
//! key itself (what `collect` stamps hardware-dependent results with and the
//! `machine-key` command prints), while [`describe_fingerprint_components`] renders
//! the individual factors that fed the hash, so a change in the key can be traced —
//! in verbose logs — to the specific hardware detail that moved.

use std::collections::BTreeMap;

use cbh_model::sanitize_segment;
use sha2::{Digest, Sha256};

/// Version tag of the fingerprint factor set, prefixed onto the canonical string.
///
/// Bumping it deliberately forks the machine key (and thus the stored series) so
/// that a change to which factors are hashed is an explicit, visible event rather
/// than a silent break in history.
const FINGERPRINT_VERSION: &str = "mk2";

/// Number of hex characters kept from the SHA-256 digest. Eight bytes (64 bits)
/// is short enough for a readable path segment yet wide enough that accidental
/// collisions between distinct hardware profiles are not a practical concern.
const FINGERPRINT_HEX_LEN: usize = 16;

/// Hardware facts that identify a machine for partitioning hardware-dependent
/// benchmark results.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareProfile {
    /// Maximum number of logical processors the system reports.
    pub processors: usize,
    /// Maximum number of NUMA memory regions the system reports.
    pub memory_regions: usize,
    /// Best-effort CPU brand string (`None` when it cannot be determined).
    pub cpu_brand: Option<String>,
    /// Histogram of the per-processor relative speeds the system reports, as
    /// `(speed, count)` pairs sorted ascending by speed.
    ///
    /// This distinguishes machines that agree on processor and memory-region
    /// counts but differ in their mix of processor speeds (for example a hybrid
    /// performance/efficiency core layout versus a uniform one).
    pub processor_speeds: Vec<(u64, usize)>,
}

/// Renders a profile to its canonical, hashable string (pure).
///
/// The factors are emitted in a fixed order as `key=value` lines, prefixed with
/// the fingerprint version. The brand is normalized so incidental whitespace
/// differences do not change the key; an absent brand contributes an empty value
/// so that machines differing only in whether a brand could be read still hash
/// consistently with their factor set. The speed histogram is rendered in its
/// fixed ascending order.
fn canonical(profile: &HardwareProfile) -> String {
    let brand = profile
        .cpu_brand
        .as_deref()
        .map(normalize_brand)
        .unwrap_or_default();
    format!(
        "{FINGERPRINT_VERSION}\nprocessors={}\nmemory_regions={}\ncpu_brand={brand}\nprocessor_speeds={}",
        profile.processors,
        profile.memory_regions,
        render_speed_histogram(&profile.processor_speeds),
    )
}

/// Collapses runs of whitespace to single spaces and trims the ends, so two
/// identical CPUs whose brand strings differ only cosmetically hash alike (pure).
fn normalize_brand(brand: &str) -> String {
    brand.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Renders a speed histogram to a canonical `speedxcount` list joined by commas,
/// for example `3141x4,6283x2` (pure). Because `HardwareProfile::processor_speeds`
/// is public, a caller may supply the pairs in any order, split one speed across
/// several entries, or include zero-count entries. Counts are therefore merged per
/// speed, zero-count speeds dropped, and the result rendered ascending by speed, so
/// that any two representations of the same histogram render — and hash —
/// identically. An empty histogram renders as an empty string.
fn render_speed_histogram(speeds: &[(u64, usize)]) -> String {
    let mut merged: BTreeMap<u64, usize> = BTreeMap::new();
    for &(speed, count) in speeds {
        if count == 0 {
            continue;
        }
        let total = merged.entry(speed).or_insert(0);
        *total = total
            .checked_add(count)
            .expect("processor count cannot exceed the usize range");
    }
    merged
        .iter()
        .map(|(speed, count)| format!("{speed}x{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Builds a speed histogram from a sequence of per-processor speeds: `(speed,
/// count)` pairs sorted ascending by speed (pure).
fn speed_histogram(speeds: impl IntoIterator<Item = u64>) -> Vec<(u64, usize)> {
    let mut counts: BTreeMap<u64, usize> = BTreeMap::new();
    for speed in speeds {
        let count = counts.entry(speed).or_insert(0);
        *count = count
            .checked_add(1)
            .expect("processor count cannot exceed the usize range");
    }
    counts.into_iter().collect()
}

/// Picks a representative CPU brand from a sequence of per-processor brands: the
/// first non-empty, whitespace-normalized brand, or `None` when none is reported
/// (pure).
fn representative_brand(brands: impl IntoIterator<Item = Option<String>>) -> Option<String> {
    brands
        .into_iter()
        .flatten()
        .map(|brand| normalize_brand(&brand))
        .find(|brand| !brand.is_empty())
}

/// The stable machine fingerprint of `profile`: the lowercase hex of the first
/// [`FINGERPRINT_HEX_LEN`] characters of the SHA-256 of its canonical string.
pub(crate) fn fingerprint(profile: &HardwareProfile) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(canonical(profile).as_bytes());
    let digest = hasher.finalize();

    let bytes_needed = FINGERPRINT_HEX_LEN.div_ceil(2);
    let mut hex = String::with_capacity(FINGERPRINT_HEX_LEN);
    for byte in digest.iter().take(bytes_needed) {
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    hex.truncate(FINGERPRINT_HEX_LEN);
    hex
}

/// Resolves the machine key for a partition: an explicit override (sanitized to a
/// single path segment) wins; otherwise the hardware [`fingerprint`] is used.
#[must_use]
pub fn resolve_machine_key(override_key: Option<&str>, profile: &HardwareProfile) -> String {
    match override_key {
        Some(key) => sanitize_segment(key),
        None => fingerprint(profile),
    }
}

/// Renders the individual factors behind a machine [`fingerprint`] as a
/// diagnostic, single-line summary (pure).
///
/// It reports the fingerprint version tag and each hardware factor using the same
/// normalized values that enter the hash, so that when a machine key changes the
/// specific factor responsible can be identified from a `--verbose` log. An absent
/// CPU brand is shown as `<none>` (matching the empty value it contributes to the
/// canonical string) rather than omitted, so its absence is itself visible.
#[must_use]
pub fn describe_fingerprint_components(profile: &HardwareProfile) -> String {
    let brand = profile.cpu_brand.as_deref().map_or_else(
        || "<none>".to_owned(),
        |brand| format!("\"{}\"", normalize_brand(brand)),
    );
    let speeds = render_speed_histogram(&profile.processor_speeds);
    let speeds = if speeds.is_empty() {
        "<none>".to_owned()
    } else {
        speeds
    };
    format!(
        "version={FINGERPRINT_VERSION}, processors={}, memory_regions={}, cpu_brand={brand}, processor_speeds={speeds}",
        profile.processors, profile.memory_regions,
    )
}

/// Profiles the host hardware (best effort).
///
/// The processor and memory-region counts, the per-processor speed histogram and
/// the representative CPU brand all come from `many_cpus`, so the fingerprint is
/// built from a single, consistent hardware source.
#[cfg_attr(test, mutants::skip)] // Queries the host; the pure logic it feeds is tested.
pub(crate) fn system_profile() -> HardwareProfile {
    let hardware = many_cpus::SystemHardware::current();
    let all_processors = hardware.all_processors();
    HardwareProfile {
        processors: hardware.max_processor_count(),
        memory_regions: hardware.max_memory_region_count(),
        cpu_brand: representative_brand(
            all_processors
                .iter()
                .map(|processor| processor.cpu_brand().map(ToOwned::to_owned)),
        ),
        processor_speeds: speed_histogram(
            all_processors
                .iter()
                .map(|processor| processor.relative_speed().as_u64()),
        ),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn profile(processors: usize, memory_regions: usize, brand: Option<&str>) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            cpu_brand: brand.map(ToOwned::to_owned),
            processor_speeds: Vec::new(),
        }
    }

    fn profile_with_speeds(
        processors: usize,
        memory_regions: usize,
        brand: Option<&str>,
        processor_speeds: Vec<(u64, usize)>,
    ) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            cpu_brand: brand.map(ToOwned::to_owned),
            processor_speeds,
        }
    }

    #[test]
    fn fingerprint_is_stable_for_a_fixed_profile() {
        // A pinned golden value proves the fingerprint is a fixed hash of the
        // canonical factor string, not a seeded or platform-varying one. If the
        // factor set, ordering, version tag, or hash changes, this must be
        // updated deliberately (and existing history is understood to fork).
        let key = fingerprint(&profile(8, 1, Some("Test CPU 3000")));
        assert_eq!(key, "bfa6cd4cc92868db");
    }

    #[test]
    fn fingerprint_is_stable_for_a_fixed_profile_with_speeds() {
        // Companion golden that pins the rendering of the speed histogram into the
        // hash, so a change to how speeds enter the key is a deliberate event.
        let key = fingerprint(&profile_with_speeds(
            4,
            1,
            Some("Test CPU 3000"),
            vec![(3141, 2), (6283, 2)],
        ));
        assert_eq!(key, "3d4b6c38a6756893");
    }

    #[test]
    fn fingerprint_is_sixteen_lowercase_hex_chars() {
        let key = fingerprint(&profile(4, 2, None));
        assert_eq!(key.len(), FINGERPRINT_HEX_LEN);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{key}"
        );
    }

    #[test]
    fn distinct_factors_produce_distinct_fingerprints() {
        let base = fingerprint(&profile(8, 1, Some("Brand A")));
        assert_ne!(base, fingerprint(&profile(16, 1, Some("Brand A"))));
        assert_ne!(base, fingerprint(&profile(8, 2, Some("Brand A"))));
        assert_ne!(base, fingerprint(&profile(8, 1, Some("Brand B"))));
        assert_ne!(base, fingerprint(&profile(8, 1, None)));
        assert_ne!(
            base,
            fingerprint(&profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 8)])),
        );
    }

    #[test]
    fn distinct_speed_histograms_produce_distinct_fingerprints() {
        let uniform = profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 8)]);
        let hybrid = profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 4), (6283, 4)]);
        assert_ne!(fingerprint(&uniform), fingerprint(&hybrid));
    }

    #[test]
    fn speed_histogram_order_does_not_change_the_fingerprint() {
        // `HardwareProfile::processor_speeds` is public, so a caller can supply the
        // same histogram with its pairs in a different order. Rendering sorts them,
        // so equivalent hardware still hashes to the same fingerprint.
        let ascending = profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 4), (6283, 4)]);
        let descending = profile_with_speeds(8, 1, Some("Brand A"), vec![(6283, 4), (3141, 4)]);
        assert_eq!(fingerprint(&ascending), fingerprint(&descending));
    }

    #[test]
    fn equivalent_speed_histogram_representations_produce_the_same_fingerprint() {
        // `HardwareProfile::processor_speeds` is public, so a caller can express the
        // same histogram as split or zero-padded entries. Rendering merges counts
        // per speed and drops zero counts, so those representations hash alike.
        let canonical = profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 4)]);
        let split = profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 1), (3141, 3)]);
        let zero_padded =
            profile_with_speeds(8, 1, Some("Brand A"), vec![(3141, 4), (6283, 0)]);
        assert_eq!(fingerprint(&canonical), fingerprint(&split));
        assert_eq!(fingerprint(&canonical), fingerprint(&zero_padded));
    }

    #[test]
    fn brand_whitespace_differences_do_not_change_the_fingerprint() {
        assert_eq!(
            fingerprint(&profile(8, 1, Some("Intel Xeon  E5"))),
            fingerprint(&profile(8, 1, Some("  Intel   Xeon E5 "))),
        );
    }

    #[test]
    fn canonical_is_versioned_and_ordered() {
        let rendered = canonical(&profile(8, 1, Some("CPU X")));
        assert_eq!(
            rendered,
            "mk2\nprocessors=8\nmemory_regions=1\ncpu_brand=CPU X\nprocessor_speeds="
        );
    }

    #[test]
    fn canonical_renders_the_speed_histogram_in_order() {
        let rendered = canonical(&profile_with_speeds(
            4,
            1,
            Some("CPU X"),
            vec![(3141, 2), (6283, 2)],
        ));
        assert_eq!(
            rendered,
            "mk2\nprocessors=4\nmemory_regions=1\ncpu_brand=CPU X\nprocessor_speeds=3141x2,6283x2"
        );
    }

    #[test]
    fn canonical_renders_absent_brand_as_empty() {
        let rendered = canonical(&profile(2, 1, None));
        assert!(rendered.contains("\ncpu_brand=\n"), "{rendered}");
    }

    #[test]
    fn normalize_brand_collapses_and_trims_whitespace() {
        assert_eq!(normalize_brand("  a   b\tc \n"), "a b c");
        assert_eq!(normalize_brand("   "), "");
    }

    #[test]
    fn resolve_machine_key_uses_fingerprint_without_override() {
        let hardware = profile(8, 1, Some("CPU X"));
        assert_eq!(resolve_machine_key(None, &hardware), fingerprint(&hardware));
    }

    #[test]
    fn resolve_machine_key_prefers_sanitized_override() {
        let hardware = profile(8, 1, Some("CPU X"));
        assert_eq!(
            resolve_machine_key(Some("ci/pool one"), &hardware),
            "ci_pool_one"
        );
    }

    #[test]
    fn describe_components_lists_version_and_normalized_factors() {
        let described = describe_fingerprint_components(&profile_with_speeds(
            8,
            1,
            Some("Intel  Xeon  E5"),
            vec![(3141, 6), (6283, 2)],
        ));
        assert_eq!(
            described,
            "version=mk2, processors=8, memory_regions=1, cpu_brand=\"Intel Xeon E5\", processor_speeds=3141x6,6283x2"
        );
    }

    #[test]
    fn describe_components_marks_absent_factors() {
        let described = describe_fingerprint_components(&profile(4, 2, None));
        assert_eq!(
            described,
            "version=mk2, processors=4, memory_regions=2, cpu_brand=<none>, processor_speeds=<none>"
        );
    }

    #[test]
    fn speed_histogram_counts_and_orders_speeds() {
        let histogram = speed_histogram([6283, 3141, 6283, 3141, 3141]);
        assert_eq!(histogram, vec![(3141, 3), (6283, 2)]);
        assert_eq!(speed_histogram(std::iter::empty()), Vec::new());
    }

    #[test]
    fn render_speed_histogram_joins_pairs_with_commas() {
        assert_eq!(
            render_speed_histogram(&[(3141, 4), (6283, 2)]),
            "3141x4,6283x2"
        );
        assert_eq!(render_speed_histogram(&[]), "");
    }

    #[test]
    fn render_speed_histogram_sorts_pairs_ascending() {
        assert_eq!(
            render_speed_histogram(&[(6283, 2), (3141, 4)]),
            "3141x4,6283x2"
        );
    }

    #[test]
    fn render_speed_histogram_merges_duplicate_speeds_and_drops_zero_counts() {
        // Duplicate speeds are summed into a single entry...
        assert_eq!(render_speed_histogram(&[(3141, 2), (3141, 2)]), "3141x4");
        // ...and zero-count speeds contribute nothing to the canonical string.
        assert_eq!(render_speed_histogram(&[(3141, 4), (6283, 0)]), "3141x4");
        assert_eq!(render_speed_histogram(&[(3141, 0)]), "");
    }

    #[test]
    fn representative_brand_picks_the_first_non_empty_normalized_brand() {
        assert_eq!(
            representative_brand([
                None,
                Some("   ".to_owned()),
                Some("  Intel  Xeon  ".to_owned())
            ]),
            Some("Intel Xeon".to_owned())
        );
        assert_eq!(representative_brand([None, Some("  ".to_owned())]), None);
        assert_eq!(
            representative_brand(std::iter::empty::<Option<String>>()),
            None
        );
    }

    #[test]
    #[cfg_attr(miri, ignore)] // Queries real hardware, which Miri cannot model.
    fn system_profile_reports_at_least_one_processor() {
        let hardware = system_profile();
        assert!(hardware.processors >= 1, "{hardware:?}");
        assert!(hardware.memory_regions >= 1, "{hardware:?}");
        // The fingerprint of whatever this machine is must be well-formed.
        assert_eq!(fingerprint(&hardware).len(), FINGERPRINT_HEX_LEN);
    }
}
