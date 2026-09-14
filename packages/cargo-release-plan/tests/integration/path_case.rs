//! Real Git/Cargo regressions for filesystem aliases and recorded path identity.
//!
//! Each case probes the relevant filesystem entries before using case aliases.
//! Reports verify release anchors and content, rather than acquisition call counts.

use std::fs;
use std::io::ErrorKind;
use std::path::MAIN_SEPARATOR;

use cargo_release_plan::{RunInput, run};
use serde_json::{Value, json};

use crate::fixture::Fixture;

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn historical_root_manifest_keeps_its_recorded_case() {
    let fixture = library_workspace();
    if !has_alias(&fixture, "cargo.toml", "Cargo.toml") {
        return;
    }
    fixture.rename_case("Cargo.toml", "cargo.toml");
    fixture.commit("release");
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn historical_member_manifest_keeps_its_recorded_case() {
    let fixture = library_workspace();
    if !has_alias(
        &fixture,
        "packages/demo/cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.rename_case("packages/demo/Cargo.toml", "packages/demo/cargo.toml");
    fixture.commit("release");
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn historical_package_listing_includes_split_directory_spellings() {
    let fixture = library_workspace();
    if !has_alias(
        &fixture,
        "packages/DEMO/Cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.git(&["add", "-A"]);
    let manifest = fixture.git(&["hash-object", "packages/demo/Cargo.toml"]);
    fixture.git(&["update-index", "--force-remove", "packages/demo/Cargo.toml"]);
    fixture.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("100644,{},packages/DEMO/Cargo.toml", manifest.trim()),
    ]);
    // Preserve the deliberate index spellings rather than restaging the filesystem view.
    fixture.git(&[
        "commit",
        "-m",
        "release with split recorded directory spellings",
    ]);
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn workspace_directory_alias_preserves_historical_members() {
    let fixture = Fixture::with_workspace_manifest(
        "Rust/Cargo.toml",
        "[workspace]\nmembers = [\"packages/demo\"]\nresolver = \"2\"\n",
    );
    library(&fixture, "Rust/packages/demo", "demo", "");
    if !has_alias(&fixture, "rust/cargo.toml", "Rust/Cargo.toml") {
        return;
    }
    fixture.rename_case("Rust", "rust");
    fixture.commit("release");
    fixture.rename_case("rust", "Rust");
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn implicit_member_alias_preserves_its_release_anchor() {
    let fixture = Fixture::with_workspace_manifest(
        "Cargo.toml",
        "[workspace]\nmembers = [\"packages/demo\"]\nresolver = \"2\"\n",
    );
    library(
        &fixture,
        "packages/demo",
        "demo",
        "[dependencies]\nhelper = { path = \"../HELPER\", version = \"0.1.0\" }\n",
    );
    library(&fixture, "packages/helper", "helper", "");
    if !has_alias(
        &fixture,
        "packages/HELPER/Cargo.toml",
        "packages/helper/Cargo.toml",
    ) {
        return;
    }
    fixture.commit("release");
    assert_unchanged(&fixture, "helper");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn case_variant_manifest_version_changes_remain_in_the_timeline() {
    let fixture = library_workspace();
    if !has_alias(
        &fixture,
        "packages/demo/cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.rename_case("packages/demo/Cargo.toml", "packages/demo/cargo.toml");
    fixture.commit("first release");
    fixture.write(
        "packages/demo/cargo.toml",
        &fixture
            .read("packages/demo/cargo.toml")
            .replace("0.1.0", "0.2.0"),
    );
    fixture.commit("second release");
    let release = fixture.sha("HEAD");
    fixture.write("unrelated.txt", "not a manifest\n");
    fixture.commit("unrelated change");
    let package = package_report(&fixture, "demo");
    assert_eq!(
        package.get("anchor").unwrap(),
        &json!({"commit": release, "version": "0.2.0"})
    );
    assert_eq!(package.get("status").unwrap(), "unchanged");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn case_variant_nested_manifest_still_draws_a_package_boundary() {
    let fixture = library_workspace();
    library(&fixture, "packages/demo/nested", "nested", "[workspace]\n");
    if !has_alias(
        &fixture,
        "packages/demo/nested/cargo.toml",
        "packages/demo/nested/Cargo.toml",
    ) {
        return;
    }
    fixture.rename_case(
        "packages/demo/nested/Cargo.toml",
        "packages/demo/nested/cargo.toml",
    );
    fixture.commit("release");
    fixture.write("packages/demo/nested/src/lib.rs", "pub fn changed() {}\n");
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn current_binary_directory_alias_keeps_lockfile_changes_relevant() {
    let fixture = binary_workspace();
    if !has_alias(
        &fixture,
        "packages/DEMO/Cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.write(
        "Cargo.toml",
        "[workspace]\nmembers = [\"packages/DEMO\"]\nresolver = \"2\"\n",
    );
    fixture.commit("release");
    fixture.write(
        "Cargo.lock",
        &fixture.read("Cargo.lock").replace("1.0.0", "1.0.1"),
    );
    let package = package_report(&fixture, "demo");
    assert_eq!(
        package.get("changed").unwrap(),
        &json!([{"source": "lockfile", "dependency": "widget", "change": "modified"}])
    );
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn historical_lockfile_alias_keeps_its_recorded_case() {
    let fixture = binary_workspace();
    if !has_alias(&fixture, "cargo.lock", "Cargo.lock") {
        return;
    }
    fixture.rename_case("Cargo.lock", "cargo.lock");
    fixture.commit("release");
    assert_unchanged(&fixture, "demo");
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn historical_registry_configuration_keeps_its_recorded_case() {
    let fixture = binary_workspace();
    fixture.write(
        "packages/demo/Cargo.toml",
        &fixture.read("packages/demo/Cargo.toml").replace(
            "widget = \"1\"",
            "widget = { version = \"1\", registry = \"private\" }",
        ),
    );
    fixture.write(
        ".cargo/Config.toml",
        "[registries.private]\nindex = \"https://example.invalid/old-index\"\n",
    );
    if !has_alias(&fixture, ".cargo/config.toml", ".cargo/Config.toml") {
        return;
    }
    fixture.write(
        "Cargo.lock",
        &fixture.read("Cargo.lock").replace(
            "https://github.com/rust-lang/crates.io-index",
            "https://example.invalid/old-index",
        ),
    );
    fixture.commit("release");
    fixture.write(
        ".cargo/Config.toml",
        &fixture
            .read(".cargo/Config.toml")
            .replace("old-index", "new-index"),
    );
    fixture.write(
        "Cargo.lock",
        &fixture.read("Cargo.lock").replace("old-index", "new-index"),
    );
    let package = package_report(&fixture, "demo");
    assert_eq!(
        package.get("changed").unwrap(),
        &json!([{"source": "lockfile", "dependency": "widget", "change": "modified"}])
    );
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn reserved_path_spelling_matches_cargo_packaging() {
    let fixture = library_workspace();
    if !has_alias(
        &fixture,
        "packages/demo/cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.write(
        "packages/demo/Cargo.toml",
        &fixture.read("packages/demo/Cargo.toml").replace(
            "edition = \"2021\"",
            "edition = \"2021\"\ninclude = [\"src/**\", \"TARGET/**\", \"cargo.lock\"]",
        ),
    );
    fixture.rename_case("packages/demo/Cargo.toml", "packages/demo/cargo.toml");
    fixture.write("packages/demo/TARGET/generated.txt", "original\n");
    fixture.write("packages/demo/cargo.lock", "version = 4\n");
    fixture.commit("release");
    let listing = fixture.cargo(&[
        "package",
        "--list",
        "--offline",
        "--allow-dirty",
        "--no-verify",
        "-p",
        "demo",
    ]);
    // Cargo's reserved-name comparisons use the listed spelling, independently
    // of whether the filesystem can open that spelling through a case alias.
    assert!(
        !listing
            .lines()
            .any(|path| path == "Cargo.toml" || path == "cargo.toml")
    );
    assert!(
        listing
            .lines()
            .any(|path| path == format!("TARGET{MAIN_SEPARATOR}generated.txt"))
    );
    assert!(listing.lines().any(|path| path == "cargo.lock"));
    fixture.write(
        "packages/demo/cargo.toml",
        &format!(
            "{}# packaged comment\n",
            fixture.read("packages/demo/cargo.toml")
        ),
    );
    fixture.write("packages/demo/TARGET/generated.txt", "changed\n");
    fixture.write("packages/demo/cargo.lock", "version = 4\n# not released\n");
    let package = package_report(&fixture, "demo");
    assert_eq!(
        package.get("changed").unwrap(),
        &json!([
            {"source": "package", "path": "TARGET/generated.txt", "change": "modified"},
            {"source": "package", "path": "cargo.lock", "change": "modified"}
        ])
    );
}

#[cfg_attr(miri, ignore = "Spawns Git and Cargo and probes the filesystem.")]
#[test]
fn resource_archive_path_uses_cargos_lexical_containment() {
    let fixture = library_workspace();
    if !has_alias(
        &fixture,
        "packages/DEMO/Cargo.toml",
        "packages/demo/Cargo.toml",
    ) {
        return;
    }
    fixture.write(
        "packages/demo/Cargo.toml",
        &fixture.read("packages/demo/Cargo.toml").replace(
            "edition = \"2021\"",
            "edition = \"2021\"\ninclude = [\"src/**\"]\nreadme = \"../DEMO/docs/readme.md\"",
        ),
    );
    fixture.write("packages/demo/docs/readme.md", "original\n");
    fixture.commit("release");
    let listing = fixture.cargo(&[
        "package",
        "--list",
        "--offline",
        "--allow-dirty",
        "--no-verify",
        "-p",
        "demo",
    ]);
    assert!(listing.lines().any(|path| path == "readme.md"));
    fixture.write("packages/demo/docs/readme.md", "changed\n");
    let package = package_report(&fixture, "demo");
    assert_eq!(
        package.get("changed").unwrap(),
        &json!([{"source": "package", "path": "readme.md", "change": "modified"}])
    );
}

fn library_workspace() -> Fixture {
    let fixture = Fixture::new("");
    library(&fixture, "packages/demo", "demo", "");
    fixture
}

fn library(fixture: &Fixture, directory: &str, name: &str, extra: &str) {
    fixture.write(
        &format!("{directory}/Cargo.toml"),
        &format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{extra}"),
    );
    fixture.write(&format!("{directory}/src/lib.rs"), "pub fn original() {}\n");
}

fn binary_workspace() -> Fixture {
    let fixture = Fixture::new("");
    library(
        &fixture,
        "packages/demo",
        "demo",
        "[dependencies]\nwidget = \"1\"\n",
    );
    fixture.write("packages/demo/src/main.rs", "fn main() {}\n");
    fixture.write(
        "Cargo.lock",
        "version = 4\n\
         [[package]]\nname = \"demo\"\nversion = \"0.1.0\"\ndependencies = [\"widget\"]\n\
         [[package]]\nname = \"widget\"\nversion = \"1.0.0\"\n\
         source = \"registry+https://github.com/rust-lang/crates.io-index\"\n",
    );
    fixture
}

fn has_alias(fixture: &Fixture, alias: &str, recorded: &str) -> bool {
    match fs::read(fixture.path().join(alias)) {
        Ok(bytes) => {
            assert_eq!(bytes, fs::read(fixture.path().join(recorded)).unwrap());
            true
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {
            eprintln!("Filesystem does not resolve {alias} as {recorded}; alias scenario not run.");
            false
        }
        Err(error) => panic!("filesystem probe failed: {error}"),
    }
}

fn package_report(fixture: &Fixture, name: &str) -> Value {
    run(&RunInput::Report {
        out_dir: fixture.path().join("out"),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let report: Value = serde_json::from_str(&fixture.read("out/report.json")).unwrap();
    report
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p.get("name").unwrap() == name)
        .unwrap()
        .clone()
}

fn assert_unchanged(fixture: &Fixture, name: &str) {
    let package = package_report(fixture, name);
    assert_eq!(
        package.get("anchor").unwrap(),
        &json!({"commit": fixture.sha("HEAD"), "version": "0.1.0"})
    );
    assert_eq!(package.get("status").unwrap(), "unchanged");
    assert_eq!(package.get("changed").unwrap(), &json!([]));
}
