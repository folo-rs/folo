//! The non-gating packaging cross-check against what Cargo would pack, and the
//! manifest resources Cargo packs from outside the package directory.

#[cfg(unix)]
use std::fs;

#[cfg(unix)]
use cargo_release_plan::{CheckFormat, RunInput, run};

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

/// Cargo dereferences a symbolic link when it packs a `.crate`, so the released
/// bytes are the target's content while Git stores only the target's path.
/// Comparing the stored paths would call the package unchanged after an edit to
/// the file the link points at, so the run stops instead of answering wrongly.
///
/// Windows cannot create a link without additional privileges, so the scenario
/// is exercised on Unix only.
#[cfg(unix)]
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_released_symbolic_link_stops_the_run() {
    let fixture = seeded_package();
    fixture.write("packages/demo/real.txt", "content\n");
    std::os::unix::fs::symlink("real.txt", fixture.path().join("packages/demo/link.txt")).unwrap();
    fixture.commit("add a link into released content");
    let base = fixture.sha("HEAD");

    let result = run(&RunInput::Check {
        base: base.clone(),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    });

    let error = result.expect_err("a released link cannot be classified");
    let message = error.to_string();
    assert!(message.contains("link.txt"), "{message}");
    assert!(message.contains("symbolic link"), "{message}");
}

/// The refusal has to hold when the link is only in history: it is the anchor
/// side that Git answers from the object database, where a link's blob is
/// indistinguishable from a small text file without consulting the tree's mode.
#[cfg(unix)]
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_symbolic_link_released_only_at_the_anchor_stops_the_run() {
    let fixture = seeded_package();
    fixture.write("packages/demo/real.txt", "content\n");
    let link = fixture.path().join("packages/demo/link.txt");
    std::os::unix::fs::symlink("real.txt", &link).unwrap();
    fixture.commit("add a link into released content");
    let base = fixture.sha("HEAD");

    // The work tree no longer holds a link, so only the anchor's tree records
    // one.
    fs::remove_file(&link).unwrap();
    fixture.write("packages/demo/link.txt", "real.txt");
    fixture.commit("replace the link with a regular file");

    let result = run(&RunInput::Check {
        base,
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    });

    let error = result.expect_err("a link at the anchor cannot be classified");
    let message = error.to_string();
    assert!(message.contains("link.txt"), "{message}");
    assert!(message.contains("symbolic link"), "{message}");
}

/// Git converts content on its way into the object database, so a work-tree
/// file and the blob recording it need not hold the same bytes. Comparing the
/// two representations directly would report every such file as modified on a
/// clean checkout, which would mark whole packages `unreleased-changes` forever.
///
/// The divergence is provoked here with a line-ending rule, which needs no
/// external tooling, but it is the same divergence Git LFS produces: this
/// repository stores `*.png` as pointer blobs while the work tree holds the
/// image.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_file_git_converts_on_the_way_in_is_not_reported_as_changed() {
    let fixture = converted_content_package();
    let base = fixture.sha("HEAD");

    let stored = fixture.git(&["show", "HEAD:packages/demo/data.txt"]);
    assert!(
        !stored.contains('\r'),
        "the fixture must actually exercise conversion, got {stored:?}"
    );

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
}

/// Conversion must not hide a real edit either: the comparison moves to Git's
/// own representation, it does not stop comparing.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_edit_to_a_converted_file_is_still_reported_as_changed() {
    let fixture = converted_content_package();
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/data.txt", "one\r\nthree\r\n");
    fixture.commit("edit converted content");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("demo: unreleased-changes"), "{message}");
}

/// Package whose released content includes a file Git rewrites as it stores it,
/// so the anchor's blob and the work-tree file hold different bytes from the
/// creation commit onwards.
fn converted_content_package() -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.write(".gitattributes", "*.txt text eol=crlf\n");
    // Written with the line endings the attribute demands in the work tree; Git
    // stores the converted form.
    fixture.write("packages/demo/data.txt", "one\r\ntwo\r\n");
    fixture.commit("seed");
    fixture
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

/// `include` never governs a file the manifest names by key: Cargo packs the
/// declared README whether or not the allow-list mentions it, so leaving it out
/// of the allow-list must not hide a change to it.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_readme_excluded_by_include_is_still_released_content() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "readme = \"README.md\"\ninclude = [\"src/**\", \"Cargo.toml\"]",
    );
    fixture.write("packages/demo/README.md", "docs\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/README.md", "revised docs\n");
    fixture.commit("revise the readme");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("demo"), "{message}");
}

/// Cargo picks a package's README off disk when the manifest names none, and
/// packs it regardless of `include`, so an allow-list that omits it must not
/// hide a change to it either.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_readme_cargo_detects_itself_is_released_content() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "demo",
        "0.1.0",
        "include = [\"src/**\", \"Cargo.toml\"]",
    );
    fixture.write("packages/demo/README.md", "docs\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    fixture.write("packages/demo/README.md", "revised docs\n");
    fixture.commit("revise the readme");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("demo"), "{message}");
}

/// The cross-check has to select released content by the same rules
/// classification uses, or a package with a README Cargo detects for itself and
/// a crate nested beneath it reports a mismatch it cannot act on.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn verify_packaging_accepts_a_detected_readme_beside_a_nested_crate() {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.write("packages/demo/README.md", "docs\n");
    fixture.write(
        "packages/demo/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    );
    fixture.write("packages/demo/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check_verifying_packaging(&fixture, &base);

    assert!(passed, "{message}");
    assert!(!message.contains("packaging rule mismatch"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_untracked_file_under_a_nested_package_is_not_advertised_for_the_outer_one() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/demo/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\npublish = false\n",
    );
    fixture.write("packages/demo/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("add a nested crate");
    fixture.write("packages/demo/inner/src/extra.rs", "pub fn h() {}\n");
    fixture.write("packages/demo/src/extra.rs", "pub fn i() {}\n");

    let report = report_json(&fixture, &base);

    assert!(report.contains("src/extra.rs"), "{report}");
    assert!(!report.contains("inner/src/extra.rs"), "{report}");
}
