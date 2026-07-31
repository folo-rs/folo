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
//!
//! The facts arrive deserialized from a stored object, so they are not guaranteed to
//! describe any machine that ever existed. Facts that do not render carry no `mk2`
//! key, which leaves the object's segment unproven — the same outcome as a segment
//! that matches neither key format, and one the caller reports rather than resolves.

use std::collections::{BTreeMap, BTreeSet};

use cbh_model::MachineInfo;
use sha2::{Digest, Sha256};

/// Version tag the legacy factor set prefixed onto its canonical string.
const LEGACY_FINGERPRINT_VERSION: &str = "mk2";

/// Number of hex characters the legacy fingerprint kept from the SHA-256 digest.
const LEGACY_FINGERPRINT_HEX_LEN: usize = 16;

/// The `mk2` machine key of the hardware facts `machine` records, or `None` when those
/// facts do not render under the retired format.
///
/// This is the key the recorded hardware hashed to before `processor_speeds` left
/// the factor set. `rekey` compares it against both the object's stored fingerprint
/// (to prove this rendering is faithful) and the object's own key segment (to prove
/// the segment is a hardware hash rather than an operator-supplied override). Facts
/// that do not render carry no such key, so neither comparison can be made and the
/// object's provenance stays unproven.
pub(crate) fn legacy_machine_key(machine: &MachineInfo) -> Option<String> {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(canonical(machine)?.as_bytes());
    let digest = hasher.finalize();

    let bytes_needed = LEGACY_FINGERPRINT_HEX_LEN.div_ceil(2);
    let mut hex = String::with_capacity(LEGACY_FINGERPRINT_HEX_LEN);
    for byte in digest.iter().take(bytes_needed) {
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    hex.truncate(LEGACY_FINGERPRINT_HEX_LEN);
    Some(hex)
}

/// Renders the recorded hardware facts to the canonical `mk2` string, or `None` when
/// a factor does not render.
///
/// The factors are emitted in a fixed order as newline-separated `key=value` pairs
/// prefixed with the version tag, with no trailing newline. `processor_speeds` is the
/// factor the current format dropped; everything else is rendered exactly as the
/// current format renders it.
fn canonical(machine: &MachineInfo) -> Option<String> {
    Some(format!(
        "{LEGACY_FINGERPRINT_VERSION}\nprocessors={}\nmemory_regions={}\nprocessor_models={}\n\
         processor_speeds={}",
        machine.processors,
        machine.memory_regions,
        render_models(&machine.processor_models),
        render_speed_histogram(&machine.processor_speeds)?,
    ))
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
/// speed — for example `3141x4,6283x2` — or `None` when the counts recorded for one
/// speed sum past `usize`.
///
/// Counts are merged per speed and zero-count speeds dropped, so any two
/// representations of the same histogram render identically. An empty histogram
/// renders as an empty string.
///
/// The counts are deserialized from a stored object rather than probed from the
/// running machine, so nothing bounds their sum: a corrupt or hand-edited record can
/// carry a histogram that no host could report. Overflow is therefore a verdict on
/// the data rather than a condition to assert against. Clamping would be worse than
/// useless here — a saturated total renders a histogram the machine never had, and
/// the key it hashes to may be a *different* machine's, which is the one outcome this
/// module exists to rule out. `None` says the recorded facts cannot be rendered
/// faithfully, leaving the object's provenance unproven.
fn render_speed_histogram(speeds: &[(u64, usize)]) -> Option<String> {
    let mut merged: BTreeMap<u64, usize> = BTreeMap::new();
    for &(speed, count) in speeds {
        if count == 0 {
            continue;
        }
        let total = merged.entry(speed).or_insert(0);
        *total = total.checked_add(count)?;
    }
    Some(
        merged
            .iter()
            .map(|(speed, count)| format!("{speed}x{count}"))
            .collect::<Vec<_>>()
            .join(","),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::collections::BTreeSet;

    use cbh_probe::resolve_machine_key;

    use super::*;
    use crate::rekey::production_hardware::{
        COBALT_100_SPLIT_CALIBRATION, COBALT_100_UNIFORM_CALIBRATION, EPYC_9V74_LINUX,
        EPYC_9V74_WINDOWS, RECORDED_RUNNERS,
    };

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
            legacy_machine_key(&machine(8, 1, &["Test CPU 3000"])).unwrap(),
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
            ))
            .unwrap(),
            "55e2e3746d2a53be"
        );
    }

    #[test]
    fn legacy_key_is_sixteen_lowercase_hex_chars() {
        let key = legacy_machine_key(&machine(4, 2, &[])).unwrap();
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
            ))
            .unwrap(),
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

        assert_eq!(
            legacy_machine_key(&reference).unwrap(),
            legacy_machine_key(&split).unwrap()
        );
        assert_eq!(
            legacy_machine_key(&reference).unwrap(),
            legacy_machine_key(&zero_padded).unwrap()
        );
        assert_eq!(
            legacy_machine_key(&ascending).unwrap(),
            legacy_machine_key(&descending).unwrap()
        );
    }

    #[test]
    fn a_histogram_whose_counts_overflow_renders_no_key() {
        // The counts are read back from a stored object, not probed, so a corrupt
        // record can claim more processors at one speed than `usize` can hold. Such
        // facts do not render, and an object whose facts do not render cannot prove
        // its key segment was an auto-detected hash.
        let speeds = vec![(3141, usize::MAX), (3141, 1)];
        assert_eq!(render_speed_histogram(&speeds), None);
        assert_eq!(
            canonical(&machine_with_speeds(8, 1, &["Model A"], speeds)),
            None
        );
        assert_eq!(
            legacy_machine_key(&machine_with_speeds(
                8,
                1,
                &["Model A"],
                vec![(3141, usize::MAX), (3141, 1)]
            )),
            None
        );
    }

    #[test]
    fn a_histogram_that_fills_usize_exactly_still_renders() {
        // The boundary sits on the renderable side: only a total that cannot be
        // represented at all leaves the facts unrenderable.
        assert_eq!(
            render_speed_histogram(&[(3141, usize::MAX - 1), (3141, 1)]),
            Some(format!("3141x{}", usize::MAX))
        );
    }

    #[test]
    fn model_order_duplicates_and_whitespace_do_not_change_the_legacy_key() {
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Model A", "Model B"])).unwrap(),
            legacy_machine_key(&machine(8, 1, &["Model B", "Model A", "Model A"])).unwrap()
        );
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Intel Xeon  E5"])).unwrap(),
            legacy_machine_key(&machine(8, 1, &[" Intel  Xeon E5 "])).unwrap()
        );
    }

    #[test]
    fn empty_model_strings_are_dropped() {
        assert_eq!(
            legacy_machine_key(&machine(8, 1, &["Model A", "", "   "])).unwrap(),
            legacy_machine_key(&machine(8, 1, &["Model A"])).unwrap()
        );
    }

    #[test]
    fn every_legacy_factor_moves_the_key() {
        let reference =
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model A"], vec![(3141, 8)])).unwrap();
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(4, 1, &["Model A"], vec![(3141, 8)])).unwrap()
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 2, &["Model A"], vec![(3141, 8)])).unwrap()
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model B"], vec![(3141, 8)])).unwrap()
        );
        assert_ne!(
            reference,
            legacy_machine_key(&machine_with_speeds(8, 1, &["Model A"], vec![(6283, 8)])).unwrap()
        );
    }

    #[test]
    fn the_legacy_rendering_reproduces_every_production_partition() {
        // Eight of the nine captured runners have their objects filed under a key the
        // retired rendering produced. Reproducing those keys from the hardware facts
        // stored beside the objects is what entitles `rekey` to claim it knows which
        // partition a stored object belongs to; without it, the migration is guessing.
        for runner in RECORDED_RUNNERS {
            let rendered = legacy_machine_key(&runner.machine())
                .unwrap_or_else(|| panic!("{}: recorded facts render a key", runner.description));
            assert_eq!(
                rendered, runner.legacy_key,
                "{}: the retired rendering now computes {rendered} for the facts stored \
                 in the live folohistory store, where production filed them under {}",
                runner.description, runner.legacy_key,
            );
        }
    }

    #[test]
    fn the_current_rendering_reproduces_every_production_destination() {
        // The other half of the mapping, computed by the live probe rather than a copy of
        // it: the partition each runner's history lands in.
        for runner in RECORDED_RUNNERS {
            let rendered = resolve_machine_key(None, &runner.profile());
            assert_eq!(
                rendered, runner.current_key,
                "{}: the current rendering now computes {rendered} where production's \
                 migration plan sends this runner's history to {}",
                runner.description, runner.current_key,
            );
        }
    }

    #[test]
    fn every_production_partition_is_one_of_the_two_renderings() {
        // A stored key is a hash of hardware facts unless an operator overrode it. Every
        // captured partition answers to one of the two renderings, so the whole capture
        // is migratable: none of it needs an operator to say where it belongs.
        for runner in RECORDED_RUNNERS {
            let legacy = legacy_machine_key(&runner.machine())
                .unwrap_or_else(|| panic!("{}: recorded facts render a key", runner.description));
            let current = resolve_machine_key(None, &runner.profile());
            assert!(
                runner.stored_key == legacy || runner.stored_key == current,
                "{}: production stores this runner under {}, which is neither the \
                 retired rendering's {legacy} nor the current rendering's {current}, so \
                 the migration cannot tell whether the segment is a hash or an override",
                runner.description,
                runner.stored_key,
            );
        }
    }

    #[test]
    fn a_production_calibration_wobble_forks_the_legacy_key_but_not_the_current_one() {
        // The observation the current key format answers to. One GitHub-hosted ARM64
        // Windows runner reports all four Cobalt 100 processors calibrated at 10678 on
        // most boots and one of the four at 10681 on others — three units in 10678, or
        // 0.028%, which is a boot-time measurement artefact and not a different machine.
        // Hashing the histogram files those boots apart; leaving it out does not.
        let uniform = &COBALT_100_UNIFORM_CALIBRATION;
        let split = &COBALT_100_SPLIT_CALIBRATION;

        assert_ne!(
            legacy_machine_key(&uniform.machine()).unwrap(),
            legacy_machine_key(&split.machine()).unwrap(),
            "the retired rendering separates the runner's two calibration readings, \
             which is why the store holds {} and {} as distinct partitions of one machine",
            uniform.stored_key,
            split.stored_key,
        );
        assert_eq!(
            resolve_machine_key(None, &uniform.profile()),
            resolve_machine_key(None, &split.profile()),
            "a calibration reading of 10681 on one of four processors instead of 10678 \
             must not move the key; it is a 0.028% measurement artefact, and letting it \
             move the key cuts the runner's history into stretches too short to judge",
        );
        assert_eq!(
            resolve_machine_key(None, &uniform.profile()),
            uniform.current_key,
            "both readings belong to the partition production's migration plan names",
        );
    }

    #[test]
    fn the_current_rendering_keeps_distinct_production_hardware_apart() {
        // Dropping a factor from a key can only merge partitions, so the question the
        // capture answers is how far it merged them. Seven runner types were spread over
        // nine partitions; the current format returns exactly seven, so it gathers the
        // readings of one machine and nothing else. It does not collapse a heterogeneous
        // cloud runner pool into a single bucket.
        let legacy: BTreeSet<String> = RECORDED_RUNNERS
            .iter()
            .map(|runner| legacy_machine_key(&runner.machine()).unwrap())
            .collect();
        let current: BTreeSet<String> = RECORDED_RUNNERS
            .iter()
            .map(|runner| resolve_machine_key(None, &runner.profile()))
            .collect();
        let models: BTreeSet<&str> = RECORDED_RUNNERS
            .iter()
            .map(|runner| runner.processor_model)
            .collect();

        assert_eq!(
            legacy.len(),
            RECORDED_RUNNERS.len(),
            "the capture holds one partition per captured reading",
        );
        assert_eq!(
            current.len(),
            models.len(),
            "the capture's {} runner readings cover {} processor models and the current \
             rendering now yields {} partitions; anything below {} means two different \
             machines share a series and their difference reads as a regression",
            RECORDED_RUNNERS.len(),
            models.len(),
            current.len(),
            models.len(),
        );
    }

    #[test]
    fn one_production_model_read_by_two_operating_systems_shares_a_partition() {
        // The AMD EPYC 9V74 runner reports its relative processor speed as 8155 under
        // Windows and 16311 under Linux — not a discrepancy but two different scales,
        // which is on its own enough to disqualify the histogram as a key factor. Both
        // readings are one machine and share a machine key.
        //
        // Their measurements stay in separate series regardless, because the target
        // triple is a facet of the discriminant set beside the machine key. That is what
        // makes dropping the histogram safe: a cross-platform comparison was never held
        // apart by the machine key in the first place.
        let windows = &EPYC_9V74_WINDOWS;
        let linux = &EPYC_9V74_LINUX;

        assert_ne!(
            windows.processor_speeds, linux.processor_speeds,
            "the two operating systems report the same processors on different scales",
        );
        assert_eq!(
            resolve_machine_key(None, &windows.profile()),
            resolve_machine_key(None, &linux.profile()),
            "one machine reported by two operating systems is one machine, and the \
             current rendering must file it as {}",
            windows.current_key,
        );
    }
}
