//! The non-gating packaging cross-check against what Cargo would pack, and the
//! manifest resources Cargo packs from outside the package directory.

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, check_verifying_packaging, report_json, seeded_package};

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

/// Cargo copies the file named by `readme` into the crate root even when it
/// lives outside the package directory, so editing a shared README republishes
/// every package that names it.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_inherited_readme_is_released_content_for_every_inheriting_package() {
    let fixture = Fixture::new("[workspace.package]\nreadme = \"README.md\"\n");
    fixture.write("README.md", "shared\n");
    write_package(&fixture, "demo", "0.1.0", "readme.workspace = true");
    write_package(&fixture, "other", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("README.md", "shared, revised\n");
    fixture.commit("revise the shared readme");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("demo"), "{message}");
    // `other` names no readme, so the same edit is not its released content.
    assert!(!message.contains("other"), "{message}");
}

/// A locally declared resource resolves against the package directory, so a
/// license file shared from a sibling directory is released content too.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_license_file_outside_the_package_directory_is_released_content() {
    let fixture = Fixture::new("");
    fixture.write("LICENSE", "terms\n");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "license-file = \"../../LICENSE\"",
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("LICENSE", "revised terms\n");
    fixture.commit("revise the license");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("demo"), "{message}");
}

/// Released content is defined from git-tracked files wherever they live, so a
/// manifest resource that is not tracked is advisory in the same way as an
/// untracked file inside the package directory.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_untracked_manifest_resource_is_advisory_only() {
    let fixture = Fixture::new("[workspace.package]\nreadme = \"README.md\"\n");
    write_package(&fixture, "demo", "0.1.0", "readme.workspace = true");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("README.md", "shared\n");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
    // Advisory, but still worth naming: it is content Cargo would pack.
    let report = report_json(&fixture, &base);
    assert!(report.contains("README.md"), "{report}");
}

/// The cross-check compares the tool's released-content set against Cargo's own
/// list, so it has to account for the resources Cargo copies in from outside the
/// package directory or every inheriting package looks like a mismatch.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_accepts_a_package_whose_readme_is_inherited() {
    let fixture = Fixture::new("[workspace.package]\nreadme = \"README.md\"\n");
    fixture.write("README.md", "shared\n");
    write_package(&fixture, "demo", "0.1.0", "readme.workspace = true");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(!message.contains("packaging rule mismatch"), "{message}");
}
