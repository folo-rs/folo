//! Exercises the helper locator's native build and filesystem boundary.

#![cfg(all(windows, not(miri)))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[test]
fn binary_path_returns_an_existing_absolute_file() {
    let resolved = dure_test_helper::binary_path();
    assert!(resolved.is_absolute(), "helper path: {resolved:?}");
    assert!(resolved.is_file(), "helper path: {resolved:?}");
}
