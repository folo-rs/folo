//! Pins the generator's local domain constants to the production pipeline's.
//!
//! The generator deliberately depends on `cbh_detect` only in dev-builds (so the `write` binary
//! can regenerate the table when `cbh_stats` does not yet compile), which means the length range,
//! regime floor, and significance gate it calibrates for are held as local copies. These tests
//! fail the moment a copy drifts from the value the detector actually uses, so a stale copy can
//! never ship a table calibrated for the wrong domain.

use cargo_bench_history_calibration::{ANCHOR_LEVEL, MAX_SERIES_LEN, MIN_REGIME, MIN_SERIES_LEN};

#[test]
fn domain_constants_match_production() {
    assert_eq!(MIN_SERIES_LEN, cbh_detect::MIN_SERIES_POINTS);
    assert_eq!(MAX_SERIES_LEN, cbh_detect::MAX_SERIES_POINTS);
    assert_eq!(MIN_REGIME, cbh_detect::MIN_REGIME);
}

#[test]
fn anchor_level_matches_the_change_gate() {
    // The grid is anchored exactly on the change gate so an adjusted p-value at the gate compares
    // against the same bits the gate uses (design.md §6.1).
    assert_eq!(
        ANCHOR_LEVEL.to_bits(),
        cbh_detect::MAX_CHANGE_CHANCE_LEVEL.to_bits()
    );
}
