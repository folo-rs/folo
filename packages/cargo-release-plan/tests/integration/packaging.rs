//! The non-gating packaging cross-check against what Cargo would pack.

use crate::fixture::{Fixture, write_package};
use crate::harness::{check_verifying_packaging, seeded_package};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_agrees_with_cargo_on_a_clean_tree() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(!message.contains("packaging rule mismatch"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_warns_without_failing_when_cargo_would_pack_an_untracked_file() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    // Untracked files are advisory only, so they are never released content —
    // but Cargo would pack this one, and naming that divergence is the whole
    // job of the cross-check.
    fixture.write("packages/demo/src/extra.rs", "pub fn g() {}\n");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    // The cross-check is advisory, so a mismatch must not change the verdict.
    assert!(passed, "{message}");
    assert!(
        message.contains("packaging rule mismatch for demo"),
        "{message}"
    );
    assert!(
        message.contains("only in `cargo package --list`: src/extra.rs"),
        "{message}"
    );
    assert!(message.contains("only in tool: nothing"), "{message}");
}

/// The packaging probe cross-checks the relevance rules against Cargo itself,
/// so a package Cargo refuses to enumerate must degrade to a warning rather
/// than turn a passing check into a failure.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_warns_when_cargo_cannot_enumerate_a_package() {
    let fixture = Fixture::new("");
    // Cargo reads the manifest for `metadata` but only resolves the referenced
    // file when it packs, so this fails the probe alone.
    write_package(&fixture, "demo", "0.1.0", "readme = \"missing.md\"");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(message.contains("packaging probe failed"), "{message}");
}
