//! Real Git/Cargo acquisition across tracked and work-tree path spellings.
//!
//! The fixture probes the filesystem before exercising case aliases. Its report
//! verifies historical acquisition and released-content identity, not source wording.

use std::fs;
use std::io::ErrorKind;

use cargo_release_plan::{RunInput, run};
use serde_json::{Value, json};

use crate::fixture::Fixture;

#[cfg_attr(
    miri,
    ignore = "Spawns Git and Cargo and probes filesystem case behavior."
)]
#[test]
fn case_aliases_preserve_the_release_anchor_and_only_report_changed_resolution() {
    let fixture = Fixture::with_workspace_manifest(
        "Rust/Cargo.toml",
        "[workspace]\nmembers = [\"packages/foo\"]\n\
         exclude = [\"packages/foo/fixture\"]\nresolver = \"2\"\n",
    );
    fixture.write(
        "Rust/packages/foo/Cargo.toml",
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
         [dependencies]\nwidget = { version = \"1\", registry = \"private\" }\n",
    );
    fixture.write("Rust/packages/foo/src/main.rs", "fn main() {}\n");
    fixture.write(
        "Rust/.cargo/Config.toml",
        "[registries.private]\nindex = \"https://example.invalid/old-index\"\n",
    );
    // No include/exclude rule on tool hides this source: its nested manifest must
    // establish the package boundary even though it is not a workspace member.
    fixture.write(
        "Rust/packages/foo/fixture/Cargo.toml",
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\n\
         edition = \"2021\"\npublish = false\n",
    );
    fixture.write(
        "Rust/packages/foo/fixture/src/lib.rs",
        "pub fn original() {}\n",
    );

    // Probe each relevant directory before recording lowercase manifest names.
    // Case sensitivity can vary within a filesystem; the OS is not evidence.
    for (alias, recorded) in [
        ("rust/cargo.toml", "Rust/Cargo.toml"),
        (
            "rust/packages/Foo/cargo.toml",
            "Rust/packages/foo/Cargo.toml",
        ),
        ("rust/.cargo/config.toml", "Rust/.cargo/Config.toml"),
        (
            "rust/packages/Foo/fixture/cargo.toml",
            "Rust/packages/foo/fixture/Cargo.toml",
        ),
    ] {
        match fs::read_to_string(fixture.path().join(alias)) {
            Ok(content) => assert_eq!(content, fixture.read(recorded)),
            Err(error) if error.kind() == ErrorKind::NotFound => {
                eprintln!(
                    "Filesystem cannot represent the case-insensitive scenario: \
                     '{alias}' does not address '{recorded}'."
                );
                return;
            }
            Err(error) => panic!("filesystem case probe failed: {error}"),
        }
    }
    for (from, to) in [
        ("Rust/Cargo.toml", "Rust/cargo.toml"),
        (
            "Rust/packages/foo/Cargo.toml",
            "Rust/packages/foo/cargo.toml",
        ),
        (
            "Rust/packages/foo/fixture/Cargo.toml",
            "Rust/packages/foo/fixture/cargo.toml",
        ),
        ("Rust", "rust"),
    ] {
        fixture.rename_case(from, to);
    }
    fixture.write(
        "rust/Cargo.lock",
        "version = 4\n\
         [[package]]\nname = \"tool\"\nversion = \"0.1.0\"\ndependencies = [\"widget\"]\n\
         [[package]]\nname = \"widget\"\nversion = \"1.0.0\"\n\
         source = \"registry+https://example.invalid/old-index\"\n",
    );
    fixture.commit("initial tool release");
    fixture.write(
        "rust/packages/foo/cargo.toml",
        &fixture
            .read("rust/packages/foo/cargo.toml")
            .replace("version = \"0.1.0\"", "version = \"0.2.0\""),
    );
    fixture.write(
        "rust/Cargo.lock",
        &fixture.read("rust/Cargo.lock").replace(
            "name = \"tool\"\nversion = \"0.1.0\"",
            "name = \"tool\"\nversion = \"0.2.0\"",
        ),
    );
    fixture.commit("release tool at its incremented version");
    let release = fixture.sha("HEAD");
    // The release is neither history endpoint; manifest-history narrowing must find it.
    fixture.write("unrelated.txt", "unrelated to the workspace\n");
    fixture.commit("unrelated repository change");

    // Keep the index's lowercase spellings: these work-tree renames are not staged.
    fixture.rename_case("rust", "Rust");
    fixture.rename_case("Rust/packages/foo", "Rust/packages/Foo");
    fixture.write(
        "Rust/.cargo/Config.toml",
        &fixture
            .read("Rust/.cargo/Config.toml")
            .replace("old-index", "new-index"),
    );
    fixture.write(
        "Rust/Cargo.lock",
        &fixture
            .read("Rust/Cargo.lock")
            .replace("old-index", "new-index"),
    );
    fixture.write(
        "Rust/packages/Foo/fixture/src/lib.rs",
        "pub fn changed() {}\n",
    );

    run(&RunInput::Report {
        out_dir: fixture.path().join("out"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let report: Value = serde_json::from_str(&fixture.read("out/report.json")).unwrap();
    let [tool] = report
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .as_slice()
    else {
        panic!("only the tool is a publishable workspace member");
    };
    assert_eq!(tool.get("name").unwrap(), "tool");
    assert_eq!(tool.get("status").unwrap(), "needs-increment");
    assert_eq!(
        tool.get("anchor").unwrap(),
        &json!({"commit": release, "version": "0.2.0"})
    );
    assert_eq!(
        tool.get("changed").unwrap(),
        &json!([{"source": "lockfile", "dependency": "widget", "change": "modified"}])
    );
}
