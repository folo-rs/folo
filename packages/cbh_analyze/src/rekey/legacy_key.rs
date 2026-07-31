//! The legacy `mk2` machine-key rendering, reimplemented for the migration alone.
//!
//! `rekey` may only move an object whose machine-key segment it can *prove* was the
//! auto-detected hardware hash, and the only proof available is to recompute the old
//! key from the object's own recorded hardware facts. That requires the retired
//! rendering rule, which is reproduced here rather than kept alive in `cbh_probe`:
//! nothing else in the tool has any use for a key format that is no longer written,
//! and a retired rule sitting beside the current one invites a caller to pick the
//! wrong one. Deleting this module deletes the last trace of `mk2`.
//!
//! The rendering is pinned by golden vectors carried over from the `mk2` revision's
//! own tests, so a drift in this reimplementation is a test failure rather than a
//! silent mis-migration.

use std::collections::{BTreeMap, BTreeSet};

use cbh_model::MachineInfo;
use sha2::{Digest, Sha256};

/// Version tag the legacy factor set prefixed onto its canonical string.
const LEGACY_FINGERPRINT_VERSION: &str = "mk2";

/// Number of hex characters the legacy fingerprint kept from the SHA-256 digest.
const LEGACY_FINGERPRINT_HEX_LEN: usize = 16;

/// The `mk2` machine key of the hardware facts `machine` records.
///
/// This is the key the recorded hardware hashed to before `processor_speeds` left
/// the factor set. `rekey` compares it against both the object's stored fingerprint
/// (to prove this rendering is faithful) and the object's own key segment (to prove
/// the segment is a hardware hash rather than an operator-supplied override).
pub(crate) fn legacy_machine_key(machine: &MachineInfo) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(canonical(machine).as_bytes());
    let digest = hasher.finalize();

    let bytes_needed = LEGACY_FINGERPRINT_HEX_LEN.div_ceil(2);
    let mut hex = String::with_capacity(LEGACY_FINGERPRINT_HEX_LEN);
    for byte in digest.iter().take(bytes_needed) {
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    hex.truncate(LEGACY_FINGERPRINT_HEX_LEN);
    hex
}

/// Renders the recorded hardware facts to the canonical `mk2` string.
///
/// The factors are emitted in a fixed order as newline-separated `key=value` pairs
/// prefixed with the version tag, with no trailing newline. `processor_speeds` is the
/// factor the current format dropped; everything else is rendered exactly as the
/// current format renders it.
fn canonical(machine: &MachineInfo) -> String {
    format!(
        "{LEGACY_FINGERPRINT_VERSION}\nprocessors={}\nmemory_regions={}\nprocessor_models={}\n\
         processor_speeds={}",
        machine.processors,
        machine.memory_regions,
        render_models(&machine.processor_models),
        render_speed_histogram(&machine.processor_speeds),
    )
}

/// Collapses runs of whitespace to single spaces and trims the ends.
fn normalize_model(model: &str) -> String {
    model.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Renders a model list to a sorted, deduplicated, comma-joined string.
///
/// Each model is whitespace-normalized and empties are dropped, so two recordings of
/// the same hardware that differ only cosmetically render identically. An empty list
/// renders as an empty string.
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

/// Renders a speed histogram to a comma-joined `speedxcount` list, ascending by
/// speed — for example `3141x4,6283x2`.
///
/// Counts are merged per speed and zero-count speeds dropped, so any two
/// representations of the same histogram render identically. An empty histogram
/// renders as an empty string.
fn render_speed_histogram(speeds: &[(u64, usize)]) -> String {
    let mut merged: BTreeMap<u64, usize> = BTreeMap::new();
    for &(speed, count) in speeds {
        if count == 0 {
            continue;
        }
        let total = merged.entry(speed).or_insert(0);
        *total = total.saturating_add(count);
    }
    merged
        .iter()
        .map(|(speed, count)| format!("{speed}x{count}"))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// Builds recorded hardware facts with no speed histogram. The stored
    /// fingerprint is irrelevant to the rendering, so it is left empty.
    fn machine(processors: usize, memory_regions: usize, models: &[&str]) -> MachineInfo {
        machine_with_speeds(processors, memory_regions, models, Vec::new())
    }

    /// Builds recorded hardware facts carrying a speed histogram.
    fn machine_with_speeds(
        processors: usize,
        memory_regions: usize,
        models: &[&str],
        processor_speeds: Vec<(u64, usize)>,
    ) -> MachineInfo {
        MachineInfo {
            processors,
            memory_regions,
            processor_models: models.iter().map(|model| (*model).to_owned()).collect(),
            processor_speeds,
            fingerprint: String::new(),
        }
    }

    #[test]
    fn legacy_key_matches_the_golden_vector_without_speeds() {
        // Carried over verbatim from the `mk2` revision's own golden test. If this
        // fails, the reimplementation has drifted from the rule that produced the
        // stored history and no object may be migrated on its word.
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Test CPU 3000"])),
            "1e3277ddba18263f"
        );
    }

    #[test]
    fn legacy_key_matches_the_golden_vector_with_speeds() {
        // The companion golden vector, pinning how the speed histogram — the factor
        // the current format drops — entered the legacy hash.
        assert_eq!(
            legacy_machine_key(&machine_with_speeds(
                4,
                1,
                &["Test CPU 3000"],
                vec![(3141, 2), (6283, 2)]
            )),
            "55e2e3746d2a53be"
        );
    }

    #[test]
    fn legacy_key_is_sixteen_lowercase_hex_chars() {
        let key = legacy_machine_key(&machine(4, 2, &[]));
        assert_eq!(key.len(), LEGACY_FINGERPRINT_HEX_LEN);
        assert!(
            key.chars()
                .all(|character| character.is_ascii_hexdigit() && !character.is_ascii_uppercase()),
            "{key}"
        );
    }

    #[test]
    fn canonical_string_names_every_legacy_factor_in_order() {
        assert_eq!(
            canonical(&machine_with_speeds(
                4,
                2,
                &["Model A"],
                vec![(3141, 4), (6283, 2)]
            )),
            "mk2\nprocessors=4\nmemory_regions=2\nprocessor_models=Model A\n\
             processor_speeds=3141x4,6283x2"
        );
    }

    #[test]
    fn speed_histogram_representation_does_not_change_the_legacy_key() {
        // The histogram is public data read back from an arbitrarily old record, so
        // the same histogram may arrive unordered, split across entries, or padded
        // with zero counts. All three must hash alike.
        let reference = machine_with_speeds(8, 1, &["Model A"], vec![(3141, 4)]);
        let split = machine_with_speeds(8, 1, &["Model A"], vec![(3141, 1), (3141, 3)]);
        let zero_padded = machine_with_speeds(8, 1, &["Model A"], vec![(3141, 4), (6283, 0)]);
        let descending = machine_with_speeds(8, 1, &["Model A"], vec![(6283, 4), (3141, 4)]);
        let ascending = machine_with_speeds(8, 1, &["Model A"], vec![(3141, 4), (6283, 4)]);

        assert_eq!(legacy_machine_key(&reference), legacy_machine_key(&split));
        assert_eq!(
            legacy_machine_key(&reference),
            legacy_machine_key(&zero_padded)
        );
        assert_eq!(
            legacy_machine_key(&ascending),
            legacy_machine_key(&descending)
        );
    }

    #[test]
    fn model_order_duplicates_and_whitespace_do_not_change_the_legacy_key() {
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Model A", "Model B"])),
            legacy_machine_key(&machine(8, 1, &["Model B", "Model A", "Model A"]))
        );
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Intel Xeon  E5"])),
            legacy_machine_key(&machine(8, 1, &[" Intel  Xeon E5 "]))
        );
    }

    #[test]
    fn empty_model_strings_are_dropped() {
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Model A", "", "   "])),
            legacy_machine_key(&machine(8, 1, &["Model A"]))
        );
    }

    #[test]
    fn every_legacy_factor_moves_the_key() {
        let reference =
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model A"], vec![(3141, 8)]));
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(4, 1, &["Model A"], vec![(3141, 8)]))
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 2, &["Model A"], vec![(3141, 8)]))
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model B"], vec![(3141, 8)]))
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model A"], vec![(6283, 8)]))
        );
    }
}
