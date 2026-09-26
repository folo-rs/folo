// Manifest inheritance syntax and acquired keys; release attribution belongs to versioning.
use std::collections::BTreeMap;

use toml_edit::{DocumentMut, Item, Value};

use crate::manifest::for_each_dependency_table;
/// Package-level keys inherited via `.workspace = true`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InheritedKeys {
    pub package: Vec<String>,
    pub dependencies: Vec<String>,
    /// Dependencies inherited exclusively through dev-dependency tables.
    ///
    /// Cargo omits these from a published manifest while their shared workspace
    /// declarations remain versionless.
    pub dev_only_dependencies: Vec<String>,
}

/// Collects `.workspace = true` keys from a package manifest.
pub fn collect_inherited_keys(doc: &DocumentMut) -> InheritedKeys {
    let mut keys = InheritedKeys::default();
    if let Some(package) = doc.get("package").and_then(Item::as_table_like) {
        for (key, item) in package.iter() {
            if is_workspace_inherit(item) {
                keys.package.push(key.to_string());
            }
        }
    }
    let mut dependency_usage = BTreeMap::new();
    for_each_dependency_table(doc.as_table(), &mut |kind, dependencies| {
        for (name, item) in dependencies.iter() {
            if !is_workspace_inherit(item) {
                continue;
            }
            let is_dev = kind == "dev-dependencies";
            dependency_usage
                .entry(name.to_string())
                .and_modify(|dev_only| *dev_only &= is_dev)
                .or_insert(is_dev);
        }
    });
    keys.package.sort();
    keys.package.dedup();
    keys.dependencies = dependency_usage.keys().cloned().collect();
    keys.dev_only_dependencies = dependency_usage
        .into_iter()
        .filter_map(|(name, dev_only)| dev_only.then_some(name))
        .collect();
    keys
}

pub fn is_workspace_inherit(item: &Item) -> bool {
    match item {
        Item::Value(Value::InlineTable(table)) => table
            .get("workspace")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        Item::Table(table) => table
            .get("workspace")
            .and_then(Item::as_bool)
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn doc(text: &str) -> DocumentMut {
        text.parse().unwrap()
    }
    #[test]
    fn collects_workspace_inherited_package_and_dep_keys() {
        let package = doc(r#"
[package]
name = "foo"
edition.workspace = true
license.workspace = true

[dependencies]
bar.workspace = true
semver = "1.0"
"#);
        let keys = collect_inherited_keys(&package);
        assert_eq!(keys.package, vec!["edition", "license"]);
        assert_eq!(keys.dependencies, vec!["bar"]);
    }

    #[test]
    fn inline_table_workspace_true_is_inherited() {
        let package = doc(r#"
[package]
name = "foo"
version = "0.1.0"

[dependencies]
bar = { workspace = true }
semver = { version = "1.0.0" }
"#);
        let keys = collect_inherited_keys(&package);
        assert_eq!(keys.dependencies, vec!["bar"]);
    }

    #[test]
    fn a_manifest_without_inheritable_tables_yields_no_keys() {
        assert!(collect_inherited_keys(&doc("")).package.is_empty());

        let odd_target = doc("[target]\nnot-a-spec = 1\n");

        assert!(collect_inherited_keys(&odd_target).dependencies.is_empty());
    }

    /// Target gated tables contribute inherited dependencies.
    ///
    /// Cargo lets a package inherit a workspace dependency from a target-gated table, and those
    /// keys must be watched the same as unconditional ones.
    #[test]
    fn target_gated_tables_contribute_inherited_dependencies() {
        let package = doc(r#"
[package]
name = "foo"

[target.'cfg(unix)'.dependencies]
bar.workspace = true

[target.'cfg(windows)'.dev-dependencies]
baz.workspace = true
"#);
        let keys = collect_inherited_keys(&package);
        assert_eq!(keys.dependencies, vec!["bar", "baz"]);
        assert_eq!(keys.dev_only_dependencies, vec!["baz"]);
    }

    #[test]
    fn dependency_used_normally_and_for_development_is_not_dev_only() {
        let package = doc("[dependencies]\nbar.workspace = true\n\
             [dev-dependencies]\nbar.workspace = true\n");

        let keys = collect_inherited_keys(&package);

        assert_eq!(keys.dependencies, vec!["bar"]);
        assert!(keys.dev_only_dependencies.is_empty());
    }
}
