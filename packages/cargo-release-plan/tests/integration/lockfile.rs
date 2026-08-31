//! Released-content consequences of the workspace lockfile.
//!
//! Cargo puts a lockfile into every package archive, but only the resolution of
//! a binary or example target is operationally relevant. Library consumers
//! resolve the library in their own dependency graph.
//! Ref: docs/design.md, "Relevant lockfile closures".

use std::fs;

use serde_json::{Value, json};

use crate::fixture::{Fixture, write_binary_package, write_package};
use crate::harness::{check, check_verbose, report_json};

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
    // `helper` publishes no binary or example target, so consumers do not use
    // the package archive's lockfile to resolve it.
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    write_lockfile(&fixture, "1.0.1");
    fixture.commit("update the locked widget");

    let (_, message) = check(&fixture, &base);
    assert!(!message.contains("helper: needs-increment"), "{message}");
}

/// An untracked conventional binary does not make the released artifact carry a lockfile.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_untracked_default_binary_does_not_require_a_lockfile() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    fixture.commit("seed library");
    let base = fixture.sha("HEAD");
    fixture.write("packages/tool/src/main.rs", "fn main() {}\n");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
}

/// An ignored conventional example does not make the released artifact carry a lockfile.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_ignored_example_does_not_require_a_lockfile() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    fixture.write(".gitignore", "packages/tool/examples/\n");
    fixture.commit("seed library");
    let base = fixture.sha("HEAD");
    fixture.write("packages/tool/examples/demo.rs", "fn main() {}\n");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
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
fn multiple_binary_packages_are_classified_from_one_lockfile() {
    let fixture = Fixture::new("");
    write_binary_package(&fixture, "first", "0.1.0", "");
    write_binary_package(&fixture, "second", "0.1.0", "");
    fixture.write(
        "Cargo.lock",
        r#"version = 4

[[package]]
name = "first"
version = "0.1.0"

[[package]]
name = "second"
version = "0.1.0"
"#,
    );
    fixture.commit("seed");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);

    assert!(passed, "{message}");
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

/// Adding the first binary starts the released lockfile closure at an empty endpoint.
///
/// The anchor is a library-only package, so no anchor lockfile is required and
/// every dependency in the work-tree closure is reported as added.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_library_that_adds_its_first_binary_needs_no_anchor_lockfile() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    write_package(&fixture, "helper", "0.1.0", "");
    fixture.commit("seed library");
    let base = fixture.sha("HEAD");

    fixture.write("packages/tool/src/main.rs", "fn main() {}\n");
    write_lockfile(&fixture, "1.0.0");
    fixture.commit("add binary");

    let (passed, message) = check_verbose(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("tool: needs-increment"), "{message}");
    assert_lockfile_change(&fixture, &base, "added");
}

/// A workspace lockfile does not invent a library-only anchor closure.
///
/// The anchor lockfile exists for another binary and happens to resolve the
/// library package too. The library's endpoint is nevertheless empty.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn an_unrelated_anchor_lockfile_does_not_create_a_library_closure() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    write_package(&fixture, "helper", "0.1.0", "");
    write_binary_package(&fixture, "runner", "0.1.0", "");
    write_lockfile(&fixture, "1.0.0");
    fixture.write(
        "Cargo.lock",
        &format!(
            "{}\n[[package]]\nname = \"runner\"\nversion = \"0.1.0\"\n",
            fixture.read("Cargo.lock")
        ),
    );
    fixture.commit("seed library beside a binary");
    let base = fixture.sha("HEAD");

    fixture.write("packages/tool/src/main.rs", "fn main() {}\n");
    fixture.commit("add binary");

    let (passed, message) = check_verbose(&fixture, &base);
    assert!(!passed, "{message}");
    assert_lockfile_change(&fixture, &base, "added");
}

/// Removing the last binary ends the released lockfile closure.
///
/// The work-tree endpoint is library-only, so it needs no lockfile and reports
/// the anchor closure as removed.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn removing_the_last_binary_needs_no_work_tree_lockfile() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");

    fs::remove_file(fixture.path().join("packages/tool/src/main.rs")).unwrap();
    fixture.write("packages/tool/src/lib.rs", "pub fn f() {}\n");
    fs::remove_file(fixture.path().join("Cargo.lock")).unwrap();
    fixture.commit("replace binary with library");

    let (passed, message) = check_verbose(&fixture, &base);
    assert!(!passed, "{message}");
    assert_lockfile_change(&fixture, &base, "deleted");
}

/// Example targets release the same lockfile closure as binary targets.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_moved_dependency_needs_an_increment_for_an_example_target() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    fixture.write("packages/tool/examples/demo.rs", "fn main() {}\n");
    write_package(&fixture, "helper", "0.1.0", "");
    write_lockfile(&fixture, "1.0.0");
    fixture.commit("seed example");
    let base = fixture.sha("HEAD");

    write_lockfile(&fixture, "1.0.1");
    fixture.commit("update the locked widget");

    let (passed, message) = check_verbose(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("tool: needs-increment"), "{message}");
    assert_lockfile_change(&fixture, &base, "modified");
}

/// Historical `src/bin/*.rs` discovery feeds lockfile classification.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_moved_dependency_needs_an_increment_for_an_auto_discovered_binary() {
    let fixture = Fixture::new("");
    write_package(&fixture, "tool", "0.1.0", "");
    fixture.write("packages/tool/src/bin/secondary.rs", "fn main() {}\n");
    write_package(&fixture, "helper", "0.1.0", "");
    write_lockfile(&fixture, "1.0.0");
    fixture.commit("seed auto-discovered binary");
    let base = fixture.sha("HEAD");

    write_lockfile(&fixture, "1.0.1");
    fixture.commit("update the locked widget");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("tool: needs-increment"), "{message}");
    assert_lockfile_change(&fixture, &base, "modified");
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
fn a_malformed_work_tree_lockfile_is_an_error() {
    let fixture = locked_workspace();
    let base = fixture.sha("HEAD");
    fixture.write("Cargo.lock", "not = = toml");

    let error = check_error(&fixture, &base);

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
        .expect_err("classification must stop when a released closure is unavailable")
}

fn assert_lockfile_change(fixture: &Fixture, base: &str, change: &str) {
    let report: Value = serde_json::from_str(&report_json(fixture, base)).unwrap();
    let changed = report
        .get("packages")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|package| package.get("name").and_then(Value::as_str) == Some("tool"))
        .and_then(|package| package.get("changed"))
        .and_then(Value::as_array)
        .unwrap();

    assert!(
        changed.contains(&json!({
            "dependency": "widget",
            "change": change,
            "source": "lockfile"
        })),
        "{changed:?}"
    );
}
