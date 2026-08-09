//! Stable machine fingerprint for partitioning benchmark results.
//!
//! The fingerprint must be reproducible across tool versions and across the
//! equivalent machines of a single pool, so it is a fixed SHA-256 over a
//! versioned, canonical string of hardware factors — never a seeded or
//! process-specific hash. Cloud machines in one pool share these factors and so
//! share a key; genuinely different hardware produces a different key. The host
//! *name* is deliberately not a factor, since pool machines have distinct names
//! but equivalent hardware.
//!
//! Only stable properties of the hardware take part: the processor and
//! memory-region counts and the distinct processor models present. Readings that
//! the same machine can report differently from one boot to the next — the
//! per-processor speed metrics above all, which are boot-time calibration figures
//! rather than hardware identity — are probed as provenance but never hashed. A
//! key that moves while the hardware stands still would fragment one machine's
//! history into incomparable pieces, which is the failure the fingerprint exists
//! to prevent.
//!
//! The key is surfaced two ways for operators. [`resolve_machine_key`] yields the
//! key itself (what `collect` stamps every result with and the
//! `machine-key` command prints), while [`describe_fingerprint_components`] renders
//! the individual factors that fed the hash, so a change in the key can be traced —
//! in verbose logs — to the specific hardware detail that moved.

use std::collections::{BTreeMap, BTreeSet};

use cbh_model::sanitize_segment;
use sha2::{Digest, Sha256};

/// Version tag of the fingerprint factor set, prefixed onto the canonical string.
///
/// Bumping it deliberately forks the machine key (and thus the stored series) so
/// that a change to which factors are hashed is an explicit, visible event rather
/// than a silent break in history.
const FINGERPRINT_VERSION: &str = "mk3";

/// Number of hex characters kept from the SHA-256 digest. Eight bytes (64 bits)
/// is short enough for a readable path segment yet wide enough that accidental
/// collisions between distinct hardware profiles are not a practical concern.
const FINGERPRINT_HEX_LEN: usize = 16;

/// Hardware facts probed from the host.
///
/// Some are fingerprint factors, identifying the machine that benchmark results are
/// partitioned by; the rest are provenance recorded beside them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HardwareProfile {
    /// Maximum number of logical processors the system reports.
    pub processors: usize,
    /// Maximum number of NUMA memory regions the system reports.
    pub memory_regions: usize,
    /// Distinct processor model strings the system reports, sorted ascending.
    ///
    /// Populated from every processor's model, then whitespace-normalized,
    /// deduplicated and sorted, so machines that agree on their set of models hash
    /// alike regardless of how many processors of each model they have.
    pub processor_models: Vec<String>,
    /// Histogram of the per-processor relative speeds the system reports, as
    /// `(speed, count)` pairs sorted ascending by speed.
    ///
    /// Provenance only — deliberately **not** a fingerprint factor. The reading
    /// behind it is a boot-time calibration figure, so the same machine can report
    /// a different value after a reboot; hashing it would split one machine's
    /// history across several keys. The processor model set already identifies
    /// what the processors are.
    pub processor_speeds: Vec<(u64, usize)>,
}

/// Renders a profile to its canonical, hashable string (pure).
///
/// The fingerprint factors are emitted in a fixed order as `key=value` lines, prefixed
/// with the fingerprint version. Models are rendered as a sorted, deduplicated,
/// comma-joined list, so machines that differ only cosmetically - in model order,
/// duplicate models, or incidental whitespace - hash consistently. An absent model set
/// contributes an empty value.
fn canonical(profile: &HardwareProfile) -> String {
    format!(
        "{FINGERPRINT_VERSION}\nprocessors={}\nmemory_regions={}\nprocessor_models={}",
        profile.processors,
        profile.memory_regions,
        render_models(&profile.processor_models),
    )
}

/// Collapses runs of whitespace to single spaces and trims the ends, so two identical
/// processors whose model strings differ only cosmetically hash alike (pure).
fn normalize_model(model: &str) -> String {
    model.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Builds a speed histogram from a sequence of per-processor speeds: `(speed,
/// count)` pairs sorted ascending by speed (pure). The result is recorded as
/// provenance and does not enter the fingerprint.
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

/// Renders a list of processor models to a canonical, comma-joined string, for example
/// `AMD EPYC,Intel Xeon` (pure). Because `HardwareProfile::processor_models` is public, a
/// caller may supply the models in any order, with duplicates, or with cosmetic whitespace
/// differences. Each model is whitespace-normalized, empties are dropped, and the distinct
/// models are sorted, so any two representations of the same model set render - and hash -
/// identically. An empty list renders as an empty string.
fn render_models(models: &[String]) -> String {
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for model in models {
        let normalized = normalize_model(model);
        if !normalized.is_empty() {
            distinct.insert(normalized);
        }
    }
    distinct.into_iter().collect::<Vec<_>>().join(",")
}

/// Reduces a sequence of per-processor models to the distinct, whitespace-normalized
/// models present, sorted ascending (pure). Empty and whitespace-only models are dropped,
/// so a machine whose processors all report the same model yields a single-entry list.
fn distinct_models(models: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut distinct: BTreeSet<String> = BTreeSet::new();
    for model in models {
        let normalized = normalize_model(&model);
        if !normalized.is_empty() {
            distinct.insert(normalized);
        }
    }
    distinct.into_iter().collect()
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
/// specific factor responsible can be identified from a `--verbose` log. Facts that the
/// profile carries as provenance rather than as factors are left out, so what is shown
/// is exactly what the key depends on. An absent model set is shown as `<none>`
/// (matching the empty value it contributes to the canonical string) rather than
/// omitted, so its absence is itself visible.
#[must_use]
pub fn describe_fingerprint_components(profile: &HardwareProfile) -> String {
    let models = render_models(&profile.processor_models);
    let models = if models.is_empty() {
        "<none>".to_owned()
    } else {
        models
    };
    format!(
        "version={FINGERPRINT_VERSION}, processors={}, memory_regions={}, \
         processor_models={models}",
        profile.processors, profile.memory_regions,
    )
}

/// Profiles the host hardware (best effort).
///
/// The processor and memory-region counts, the distinct processor models and the
/// per-processor speed histogram all come from `many_cpus`, so the fingerprint and the
/// provenance recorded beside it are built from a single, consistent hardware source.
#[cfg_attr(test, mutants::skip)] // Queries the host; the pure logic it feeds is tested.
pub(crate) fn system_profile() -> HardwareProfile {
    let hardware = many_cpus::SystemHardware::current();
    let all_processors = hardware.all_processors();
    HardwareProfile {
        processors: hardware.max_processor_count(),
        memory_regions: hardware.max_memory_region_count(),
        processor_models: distinct_models(
            all_processors
                .iter()
                .map(|processor| processor.model().to_owned()),
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

    fn profile(processors: usize, memory_regions: usize, models: &[&str]) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            processor_models: models.iter().map(|model| (*model).to_owned()).collect(),
            processor_speeds: Vec::new(),
        }
    }

    fn profile_with_speeds(
        processors: usize,
        memory_regions: usize,
        models: &[&str],
        processor_speeds: Vec<(u64, usize)>,
    ) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            processor_models: models.iter().map(|model| (*model).to_owned()).collect(),
            processor_speeds,
        }
    }

    #[test]
    fn fingerprint_is_stable_for_a_fixed_profile() {
        // A pinned golden value proves the fingerprint is a fixed hash of the
        // canonical factor string, not a seeded or platform-varying one. If the
        // factor set, ordering, version tag, or hash changes, this must be
        // updated deliberately (and existing history is understood to fork).
        let key = fingerprint(&profile(8, 1, &["Test CPU 3000"]));
        assert_eq!(key, "4ffd697efb295d32");
    }

    #[test]
    fn fingerprint_is_sixteen_lowercase_hex_chars() {
        let key = fingerprint(&profile(4, 2, &[]));
        assert_eq!(key.len(), FINGERPRINT_HEX_LEN);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{key}"
        );
    }

    #[test]
    fn distinct_factors_produce_distinct_fingerprints() {
        let base = fingerprint(&profile(8, 1, &["Model A"]));
        assert_ne!(base, fingerprint(&profile(16, 1, &["Model A"])));
        assert_ne!(base, fingerprint(&profile(8, 2, &["Model A"])));
        assert_ne!(base, fingerprint(&profile(8, 1, &["Model B"])));
        assert_ne!(base, fingerprint(&profile(8, 1, &["Model A", "Model B"])));
        assert_ne!(base, fingerprint(&profile(8, 1, &[])));
    }

    #[test]
    fn processor_speeds_do_not_change_the_fingerprint() {
        // The speed reading is a boot-time calibration figure, so one machine can
        // report a different histogram after a reboot. Hardware that agrees on the
        // real factors must therefore land on one key however its speeds read,
        // otherwise a single runner's history fragments across keys.
        let absent = profile(8, 1, &["Model A"]);
        let uniform = profile_with_speeds(8, 1, &["Model A"], vec![(3141, 8)]);
        let drifted = profile_with_speeds(8, 1, &["Model A"], vec![(3142, 8)]);
        let hybrid = profile_with_speeds(8, 1, &["Model A"], vec![(3141, 4), (6283, 4)]);
        assert_eq!(fingerprint(&absent), fingerprint(&uniform));
        assert_eq!(fingerprint(&absent), fingerprint(&drifted));
        assert_eq!(fingerprint(&absent), fingerprint(&hybrid));
    }

    #[test]
    fn model_order_and_duplicates_do_not_change_the_fingerprint() {
        // `HardwareProfile::processor_models` is public, so a caller can supply the same
        // models in a different order or with duplicates. Rendering sorts and dedups them,
        // so equivalent hardware still hashes to the same fingerprint.
        let ascending = profile(8, 1, &["Model A", "Model B"]);
        let shuffled = profile(8, 1, &["Model B", "Model A", "Model A"]);
        assert_eq!(fingerprint(&ascending), fingerprint(&shuffled));
    }

    #[test]
    fn model_whitespace_differences_do_not_change_the_fingerprint() {
        assert_eq!(
            fingerprint(&profile(8, 1, &["Intel Xeon  E5"])),
            fingerprint(&profile(8, 1, &["  Intel   Xeon E5 "])),
        );
    }

    #[test]
    fn canonical_is_versioned_and_ordered() {
        let rendered = canonical(&profile(8, 1, &["CPU X"]));
        assert_eq!(
            rendered,
            "mk3\nprocessors=8\nmemory_regions=1\nprocessor_models=CPU X"
        );
    }

    #[test]
    fn canonical_ignores_the_provenance_speed_histogram() {
        let rendered = canonical(&profile_with_speeds(
            8,
            1,
            &["CPU X"],
            vec![(3141, 2), (6283, 6)],
        ));
        assert_eq!(
            rendered,
            "mk3\nprocessors=8\nmemory_regions=1\nprocessor_models=CPU X"
        );
    }

    #[test]
    fn canonical_renders_absent_models_as_empty() {
        let rendered = canonical(&profile(2, 1, &[]));
        assert!(rendered.ends_with("\nprocessor_models="), "{rendered}");
    }

    #[test]
    fn normalize_model_collapses_and_trims_whitespace() {
        assert_eq!(normalize_model("  a   b\tc \n"), "a b c");
        assert_eq!(normalize_model("   "), "");
    }

    #[test]
    fn resolve_machine_key_uses_fingerprint_without_override() {
        let hardware = profile(8, 1, &["CPU X"]);
        assert_eq!(resolve_machine_key(None, &hardware), fingerprint(&hardware));
    }

    #[test]
    fn resolve_machine_key_prefers_sanitized_override() {
        let hardware = profile(8, 1, &["CPU X"]);
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
            &["Intel  Xeon  E5", "AMD EPYC"],
            vec![(3141, 6), (6283, 2)],
        ));
        // The speed histogram the profile carries as provenance is not a factor, so
        // it must not surface among the components the key depends on.
        assert_eq!(
            described,
            "version=mk3, processors=8, memory_regions=1, \
             processor_models=AMD EPYC,Intel Xeon E5"
        );
    }

    #[test]
    fn describe_components_marks_absent_factors() {
        let described = describe_fingerprint_components(&profile(4, 2, &[]));
        assert_eq!(
            described,
            "version=mk3, processors=4, memory_regions=2, processor_models=<none>"
        );
    }

    #[test]
    fn speed_histogram_counts_and_orders_speeds() {
        let histogram = speed_histogram([6283, 3141, 6283, 3141, 3141]);
        assert_eq!(histogram, vec![(3141, 3), (6283, 2)]);
        assert_eq!(speed_histogram(std::iter::empty()), Vec::new());
    }

    #[test]
    fn distinct_models_normalizes_sorts_and_dedups() {
        // Raw per-processor models arrive in arbitrary order, with duplicates, cosmetic
        // whitespace and empty entries; the distinct set is normalized and sorted.
        let models = distinct_models([
            "  Intel  Xeon  ".to_owned(),
            "AMD EPYC".to_owned(),
            "Intel Xeon".to_owned(),
            String::new(),
            "   ".to_owned(),
        ]);
        assert_eq!(models, vec!["AMD EPYC".to_owned(), "Intel Xeon".to_owned()]);
        assert_eq!(
            distinct_models(std::iter::empty::<String>()),
            Vec::<String>::new()
        );
    }

    #[test]
    fn render_models_joins_sorted_distinct_with_commas() {
        assert_eq!(
            render_models(&["Intel Xeon".to_owned(), "AMD EPYC".to_owned()]),
            "AMD EPYC,Intel Xeon"
        );
        // Duplicates and cosmetic whitespace collapse to a single entry.
        assert_eq!(
            render_models(&["AMD EPYC".to_owned(), "  AMD   EPYC ".to_owned()]),
            "AMD EPYC"
        );
        assert_eq!(render_models(&[]), "");
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

    #[test]
    fn a_production_arm_runner_keeps_one_key_across_its_calibration_readings() {
        // Hardware captured from the live `folohistory` benchmark store on 2026-08-01.
        //
        // These are the two readings a single GitHub-hosted ARM64 Windows runner produces:
        // on most boots all four Cobalt 100 processors report a relative-speed calibration
        // of 10678, and on others one of the four reports 10681 instead — a difference of
        // three units in 10678, or 0.028%, with nothing about the machine changed. The
        // key is what a series is accumulated under, so a factor that a reboot can move by
        // a fraction of a percent cannot be one: the runner would file its history under a
        // fresh key every time the reading wobbled, leaving no stretch long enough to
        // judge a regression against.
        let uniform = profile_with_speeds(4, 1, &["Cobalt 100"], vec![(10678, 4)]);
        let recalibrated = profile_with_speeds(4, 1, &["Cobalt 100"], vec![(10678, 3), (10681, 1)]);

        assert_eq!(
            canonical(&uniform),
            "mk3\nprocessors=4\nmemory_regions=1\nprocessor_models=Cobalt 100",
        );
        assert_eq!(
            canonical(&recalibrated),
            canonical(&uniform),
            "the 10681 reading is provenance, so it must leave the factor string alone",
        );
        assert_eq!(
            resolve_machine_key(None, &uniform),
            "2e3ad42f4e2cd3e1",
            "this runner's production history is filed under this key",
        );
        assert_eq!(
            resolve_machine_key(None, &recalibrated),
            resolve_machine_key(None, &uniform),
            "one machine's two calibration readings are one machine",
        );
    }
}
