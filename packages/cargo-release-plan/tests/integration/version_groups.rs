//! Cross-checks the two declarations of the workspace's version groups.
//!
//! Version groups are declared twice while `release-plz update` is still the
//! mechanism that raises versions: once as
//! `[workspace.metadata.release-plan.groups]` in the workspace manifest, which
//! this tool reads, and once as `version_group` keys in `release-plz.toml`. Two
//! declarations of one fact drift silently, and a drifted pair would have the
//! two tools disagree about which packages must move together.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item};

/// The two version-group declarations agree.
///
/// Neither tool can detect the other's view, so an entry added to one file and
/// forgotten in the other would silently split a group until a release exposed
/// it.
#[cfg_attr(miri, ignore)] // Reads the workspace's own manifests from disk.
#[test]
fn the_two_version_group_declarations_agree() {
    let root = workspace_root();
    let plan = release_plan_groups(&root.join("Cargo.toml"));
    let plz = release_plz_groups(&root.join("release-plz.toml"));

    // Both readers returning nothing would satisfy the comparison while proving
    // nothing, so the workspace's own groups must be visible to both.
    assert!(!plan.is_empty());
    assert_eq!(plan, plz);
}

/// Directory holding the workspace manifest.
///
/// The test runs with its own package directory as the working directory, and
/// this package sits one level below `packages/`.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .expect("this package is nested two levels below the workspace root")
        .to_path_buf()
}

/// Groups as `cargo-release-plan` reads them from the workspace manifest.
fn release_plan_groups(manifest: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let document = parse(manifest);
    let groups = document
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("metadata"))
        .and_then(Item::as_table_like)
        .and_then(|metadata| metadata.get("release-plan"))
        .and_then(Item::as_table_like)
        .and_then(|plan| plan.get("groups"))
        .and_then(Item::as_table_like)
        .expect("the workspace manifest declares version groups");

    groups
        .iter()
        .map(|(name, members)| {
            let members = members
                .as_array()
                .expect("a version group is an array of package names")
                .iter()
                .map(|member| {
                    member
                        .as_str()
                        .expect("a group member is a package name")
                        .to_string()
                })
                .collect();
            (name.to_string(), members)
        })
        .collect()
}

/// Groups as release-plz reads them from its own configuration.
fn release_plz_groups(config: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let document = parse(config);
    let packages = document
        .get("package")
        .and_then(Item::as_array_of_tables)
        .expect("release-plz configures packages individually");

    let mut groups: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for package in packages {
        let Some(group) = package.get("version_group").and_then(Item::as_str) else {
            continue;
        };
        let name = package
            .get("name")
            .and_then(Item::as_str)
            .expect("a release-plz package entry names its package");
        groups
            .entry(group.to_string())
            .or_default()
            .insert(name.to_string());
    }
    groups
}

fn parse(path: &Path) -> DocumentMut {
    fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
        .parse()
        .unwrap_or_else(|error| panic!("parsing {}: {error}", path.display()))
}
