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
