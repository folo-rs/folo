//! Package boundaries when one package's directory contains another.

use std::fs;

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, check_workspace, nested_workspace};

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn nested_package_content_belongs_only_to_the_nested_package() {
    let fixture = nested_workspace();
    let base = fixture.sha("HEAD");
    fixture.write(
        "packages/outer/inner/src/lib.rs",
        "pub fn g() { let _ = 1; }\n",
    );
    fixture.commit("change inner");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    // `inner` is a member only because `outer` depends on it by path, so it must
    // be reconstructed at the anchor too, or the change would look like a brand
    // new package. Its files belong to `inner` alone, so `outer` stays unchanged.
    assert!(message.contains("inner: needs-increment"), "{message}");
    assert!(!message.contains("outer: needs-increment"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn nested_package_boundary_holds_when_the_workspace_is_below_the_repository_root() {
    let fixture = Fixture::new("");
    fixture.write(
        "sub/Cargo.toml",
        "[workspace]\nmembers = [\"packages/*\"]\nresolver = \"2\"\n",
    );
    fixture.write(
        "sub/packages/outer/Cargo.toml",
        concat!(
            "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[dependencies]\ninner = { path = \"inner\", version = \"0.1.0\" }\n"
        ),
    );
    fixture.write("sub/packages/outer/src/lib.rs", "pub fn f() {}\n");
    fixture.write(
        "sub/packages/outer/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("sub/packages/outer/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed nested workspace");
    let base = fixture.sha("HEAD");

    fixture.write(
        "sub/packages/outer/inner/src/lib.rs",
        "pub fn g() { let _ = 6; }\n",
    );
    fixture.commit("change inner");

    // The workspace root is below the repository root, so member directories and
    // package directories both have to be expressed relative to the repository
    // before the nested-package boundary is applied.
    let (passed, message) = check_workspace(&base, fixture.path().join("sub").join("Cargo.toml"));
    assert!(!passed, "{message}");
    assert!(message.contains("inner: needs-increment"), "{message}");
    assert!(!message.contains("outer: needs-increment"), "{message}");
}

/// A path dependency without a manifest at the anchor is not a member there.
///
/// A path dependency is followed to reconstruct membership at the anchor, where the target
/// directory may not have carried a manifest yet. The directory's files then belonged to the
/// depending package, so turning it into a package takes them out of what the outer one releases.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_path_dependency_without_a_manifest_at_the_anchor_is_not_a_member_there() {
    let fixture = Fixture::new("");
    fixture.write(
        "packages/outer/Cargo.toml",
        concat!(
            "[package]\nname = \"outer\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n",
            "[dependencies]\ninner = { path = \"inner\", version = \"0.1.0\" }\n"
        ),
    );
    fixture.write("packages/outer/src/lib.rs", "pub fn f() {}\n");
    fixture.write("packages/outer/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed with the dependency target still manifest-less");

    fixture.write(
        "packages/outer/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.commit("give the target a manifest");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);

    assert!(!passed, "{message}");
    assert!(message.contains("outer: needs-increment"), "{message}");
    assert!(message.contains("inner/src/lib.rs"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_nested_manifest_the_work_tree_deleted_no_longer_stops_packing() {
    let fixture = Fixture::new("exclude = [\"packages/demo/fixture\"]");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.write(
        "packages/demo/fixture/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("packages/demo/fixture/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    // Git keeps listing a tracked file the work tree has deleted, but Cargo
    // packages what is on disk: with the nested manifest gone, `demo` now packs
    // the files beneath it and they are content it has never released.
    fs::remove_file(fixture.path().join("packages/demo/fixture/Cargo.toml")).unwrap();

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("demo: needs-increment"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn nested_package_boundary_holds_for_a_crate_that_is_not_a_member() {
    let fixture = Fixture::new("exclude = [\"packages/demo/fixture\"]");
    write_package(&fixture, "demo", "0.1.0", "");
    // Excluded from the workspace and depended on by nobody, so it appears in no
    // member list, yet Cargo still stops packing `demo` at its manifest.
    fixture.write(
        "packages/demo/fixture/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("packages/demo/fixture/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    fixture.write(
        "packages/demo/fixture/src/lib.rs",
        "pub fn g() { let _ = 7; }\n",
    );
    fixture.commit("change the excluded nested package");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}
