//! Released-content consequences of the workspace lockfile.
//!
//! Cargo puts a lockfile into every archive it builds and `cargo install
//! --locked` builds an executable from it, so a package with a binary target
//! releases its resolved dependency closure while a library does not.
//! Ref: docs/design.md, "Lockfiles of binary packages".

use std::fs;

use crate::fixture::{Fixture, write_binary_package, write_package};
use crate::harness::{check, check_verbose};

/// Writes a workspace lockfile resolving `tool` onto `helper` and `widget`.
///
/// The lockfile is written by hand rather than resolved, because a test that
/// let Cargo resolve one would need a registry. Only `widget`'s version varies
/// between the two revisions a test compares, so the closure is the only thing
/// that can explain a verdict.
fn write_lockfile(fixture: &Fixture, widget_version: &str) {
    fixture.write(
        "Cargo.lock",
        &format!(
            r#"version = 4

[[package]]
name = "helper"
version = "0.1.0"

[[package]]
name = "tool"
version = "0.1.0"
dependencies = [
 "helper",
 "widget",
]

[[package]]
name = "widget"
version = "{widget_version}"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "1111111111111111111111111111111111111111111111111111111111111111"
"#
        ),
    );
}

fn locked_workspace() -> Fixture {
    let fixture = Fixture::new("");
    write_binary_package(&fixture, "tool", "0.1.0", "");
    write_package(&fixture, "helper", "0.1.0", "");
    write_lockfile(&fixture, "1.0.0");
    fixture.commit("seed");
    fixture
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_moved_dependency_needs_an_increment_for_a_binary_package() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    write_lockfile(&fixture, "1.0.1");
    fixture.commit("update the locked widget");

    let (passed, message) = check_verbose(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("tool: needs-increment"), "{message}");
    assert!(message.contains("widget"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_moved_dependency_leaves_a_library_package_unchanged() {
    // `helper` publishes no executable, so nothing a consumer builds against
    // comes from the lockfile even though the archive still carries one.
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    write_lockfile(&fixture, "1.0.1");
    fixture.commit("update the locked widget");

    let (_, message) = check(&fixture, &base);
    assert!(!message.contains("helper: needs-increment"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_incremented_binary_package_settles() {
    // The package's own entry is not part of its closure, so incrementing it
    // must not register as a further change to its dependencies.
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    write_binary_package(&fixture, "tool", "0.2.0", "");
    fixture.write(
        "Cargo.lock",
        &fixture.read("Cargo.lock").replace(
            "name = \"tool\"\nversion = \"0.1.0\"",
            "name = \"tool\"\nversion = \"0.2.0\"",
        ),
    );
    fixture.commit("increment tool");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_untouched_lockfile_leaves_a_binary_package_unchanged() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    fixture.write("packages/helper/src/lib.rs", "pub fn f() { let _ = 3; }\n");
    fixture.commit("edit helper only");

    let (_, message) = check(&fixture, &base);
    assert!(!message.contains("tool: needs-increment"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_new_binary_package_needs_no_historical_lockfile() {
    let fixture = Fixture::new("");
    write_package(&fixture, "existing", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");
    write_binary_package(&fixture, "tool", "0.1.0", "");
    fixture.commit("add binary package");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_binary_anchor_without_a_lockfile_is_an_error() {
    let fixture = Fixture::new("");
    write_binary_package(&fixture, "tool", "0.1.0", "");
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    fixture.write("packages/tool/src/main.rs", "fn main() { let _ = 4; }\n");
    fixture.commit("edit the binary");

    let error = check_error(&fixture, &base);
    assert!(error.contains("anchor commit"), "{error}");
    assert!(error.contains("Cargo.lock"), "{error}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_missing_work_tree_lockfile_is_an_error() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");
    fs::remove_file(fixture.path().join("Cargo.lock")).unwrap();

    let error = check_error(&fixture, &base);
    assert!(error.contains("work tree"), "{error}");
    assert!(error.contains("Cargo.lock"), "{error}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_anchor_lockfile_that_does_not_resolve_the_binary_is_an_error() {
    let fixture = Fixture::new("");
    write_binary_package(&fixture, "tool", "0.1.0", "");
    fixture.write(
        "Cargo.lock",
        r#"version = 4

[[package]]
name = "widget"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    fixture.write("packages/tool/src/main.rs", "fn main() { let _ = 4; }\n");
    fixture.commit("edit the binary");

    let error = check_error(&fixture, &base);
    assert!(error.contains("anchor Cargo.lock"), "{error}");
    assert!(error.contains("declared version"), "{error}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_work_tree_lockfile_that_does_not_resolve_the_binary_is_an_error() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");
    fixture.write(
        "Cargo.lock",
        r#"version = 4

[[package]]
name = "widget"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#,
    );

    let error = check_error(&fixture, &base);
    assert!(error.contains("work-tree Cargo.lock"), "{error}");
    assert!(error.contains("declared version"), "{error}");
}

fn check_error(fixture: &Fixture, base: &str) -> String {
    crate::harness::check_result(fixture, base)
        .expect_err("classification must stop when a binary closure is unavailable")
}
