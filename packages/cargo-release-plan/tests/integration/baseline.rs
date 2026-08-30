//! Discovering the release baseline when the caller names none.
//!
//! Ref: docs/design.md, "The release baseline".

use crate::fixture::{Fixture, write_package};
use crate::harness::check_discovering_base;

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn the_recorded_remote_head_supplies_the_baseline() {
    // The remote head names a branch other than `main`, so a run that resolved
    // the convention instead would not find this baseline at all.
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    let seed = fixture.sha("HEAD");
    fixture.git(&["update-ref", "refs/remotes/origin/trunk", &seed]);
    fixture.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);

    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 2; }\n");
    fixture.commit("content without an increment");

    let (passed, message) = check_discovering_base(&fixture).unwrap();
    assert!(!passed, "{message}");
    assert!(message.contains("demo: unreleased-changes"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_unrecorded_remote_head_falls_back_to_the_convention() {
    // Nothing records a remote head here, so the run reaches for the
    // conventional name and reports that it is the revision it could not find.
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");

    let error = check_discovering_base(&fixture).unwrap_err();
    assert!(error.contains("origin/main"), "{error}");
}
