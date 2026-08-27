// Attribution of inherited workspace values to packages.
//
// A root-manifest edit is in scope for a package only when that package actually
// inherits the changed `[workspace.package]` key or `[workspace.dependencies]`
// entry. `[workspace.lints]` is out of scope.

use std::collections::{BTreeMap, BTreeSet};

use toml_edit::{DocumentMut, Item, TableLike, Value};

/// One inherited field that changed between the package's anchor and the work tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InheritedChange {
    pub(crate) field: String,
}

/// Package-level keys inherited via `.workspace = true`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct InheritedKeys {
    pub(crate) package: Vec<String>,
    pub(crate) dependencies: Vec<String>,
}

/// Compares inherited workspace values at the package's anchor vs the work tree.
pub(crate) fn inherited_changes(
    keys: &InheritedKeys,
    workspace_at_anchor: &DocumentMut,
    workspace_at_work_tree: &DocumentMut,
) -> Vec<InheritedChange> {
    let mut changes = Vec::new();

    for key in &keys.package {
        let old = workspace_package_value(workspace_at_anchor, key);
        let new = workspace_package_value(workspace_at_work_tree, key);
        if old != new {
            changes.push(InheritedChange {
                field: format!("workspace.package.{key}"),
            });
        }
    }

    for dep in &keys.dependencies {
        let old = workspace_dependency_fields(workspace_at_anchor, dep);
        let new = workspace_dependency_fields(workspace_at_work_tree, dep);
        if old == new {
            continue;
        }
        let names: Vec<String> = old
            .keys()
            .chain(new.keys())
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for field in &names {
            if old.get(field) != new.get(field) {
                changes.push(InheritedChange {
                    field: format!("workspace.dependencies.{dep}.{field}"),
                });
            }
        }
        if names.is_empty() {
            changes.push(InheritedChange {
                field: format!("workspace.dependencies.{dep}"),
            });
        }
    }

    changes
}

/// Collects `.workspace = true` keys from a package manifest.
pub(crate) fn collect_inherited_keys(doc: &DocumentMut) -> InheritedKeys {
    let mut keys = InheritedKeys::default();
    if let Some(package) = doc.get("package").and_then(Item::as_table_like) {
        for (key, item) in package.iter() {
            if is_workspace_inherit(item) {
                keys.package.push(key.to_string());
            }
        }
    }
    for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(table_name).and_then(Item::as_table_like) {
            collect_inherited_deps(table, &mut keys.dependencies);
        }
    }
    if let Some(target) = doc.get("target").and_then(Item::as_table_like) {
        for (_, spec) in target.iter() {
            if let Some(spec) = spec.as_table_like() {
                for table_name in ["dependencies", "dev-dependencies", "build-dependencies"] {
                    if let Some(table) = spec.get(table_name).and_then(Item::as_table_like) {
                        collect_inherited_deps(table, &mut keys.dependencies);
                    }
                }
            }
        }
    }
    keys.package.sort();
    keys.package.dedup();
    keys.dependencies.sort();
    keys.dependencies.dedup();
    keys
}

fn collect_inherited_deps(table: &dyn TableLike, out: &mut Vec<String>) {
    for (name, item) in table.iter() {
        if is_workspace_inherit(item) {
            out.push(name.to_string());
        }
    }
}

pub(crate) fn workspace_package_version(doc: &DocumentMut) -> Option<&str> {
    doc.get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("package"))
        .and_then(Item::as_table_like)
        .and_then(|package| package.get("version"))
        .and_then(Item::as_str)
}

pub(crate) fn is_workspace_inherit(item: &Item) -> bool {
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

fn workspace_package_value(doc: &DocumentMut, key: &str) -> Option<String> {
    let package = doc
        .get("workspace")?
        .as_table_like()?
        .get("package")?
        .as_table_like()?;
    Some(fingerprint(package.get(key)?))
}

fn workspace_dependency_fields(doc: &DocumentMut, name: &str) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    let Some(deps) = doc
        .get("workspace")
        .and_then(Item::as_table_like)
        .and_then(|workspace| workspace.get("dependencies"))
        .and_then(Item::as_table_like)
    else {
        return fields;
    };
    let Some(item) = deps.get(name) else {
        return fields;
    };
    match item {
        Item::Value(Value::String(s)) => {
            fields.insert("version".to_string(), s.value().clone());
        }
        Item::Value(Value::InlineTable(table)) => {
            for (key, value) in table {
                fields.insert(key.to_string(), value_fingerprint(value));
            }
        }
        Item::Table(table) => {
            for (key, value) in table {
                fields.insert(key.to_string(), fingerprint(value));
            }
        }
        other => {
            fields.insert("value".to_string(), fingerprint(other));
        }
    }
    fields
}

fn fingerprint(item: &Item) -> String {
    match item {
        Item::Value(value) => value_fingerprint(value),
        Item::Table(table) => {
            let mut parts: Vec<String> = table
                .iter()
                .map(|(k, v)| format!("{k}={}", fingerprint(v)))
                .collect();
            parts.sort();
            parts.join(",")
        }
        Item::ArrayOfTables(array) => array
            .iter()
            .map(|table| fingerprint(&Item::Table(table.clone())))
            .collect::<Vec<_>>()
            .join(";"),
        Item::None => String::new(),
    }
}

fn value_fingerprint(value: &Value) -> String {
    match value {
        Value::String(s) => s.value().clone(),
        Value::Integer(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::Datetime(d) => d.to_string(),
        Value::Array(array) => array
            .iter()
            .map(value_fingerprint)
            .collect::<Vec<_>>()
            .join(","),
        Value::InlineTable(table) => {
            let mut parts: Vec<String> = table
                .iter()
                .map(|(k, v)| format!("{k}={}", value_fingerprint(v)))
                .collect();
            parts.sort();
            parts.join(",")
        }
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
    fn workspace_package_version_reads_workspace_package_table() {
        let with_version = doc("[workspace.package]\nversion = \"1.2.3\"\n");
        assert_eq!(workspace_package_version(&with_version), Some("1.2.3"));
        let missing = doc("[workspace]\n");
        assert_eq!(workspace_package_version(&missing), None);
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
    fn attributes_changed_workspace_package_key() {
        let keys = InheritedKeys {
            package: vec!["edition".to_string()],
            dependencies: vec![],
        };
        let old = doc("[workspace.package]\nedition = \"2021\"\n");
        let new = doc("[workspace.package]\nedition = \"2024\"\n");
        let changes = inherited_changes(&keys, &old, &new);
        assert_eq!(
            changes,
            vec![InheritedChange {
                field: "workspace.package.edition".to_string(),
            }]
        );
    }

    #[test]
    fn ignores_uninherited_workspace_package_key() {
        let keys = InheritedKeys::default();
        let old = doc("[workspace.package]\nedition = \"2021\"\n");
        let new = doc("[workspace.package]\nedition = \"2024\"\n");
        assert!(inherited_changes(&keys, &old, &new).is_empty());
    }

    #[test]
    fn attributes_changed_workspace_dependency_version() {
        let keys = InheritedKeys {
            package: vec![],
            dependencies: vec!["bar".to_string()],
        };
        let old = doc(
            "[workspace.dependencies]\nbar = { version = \"1.0.0\", path = \"packages/bar\" }\n",
        );
        let new = doc(
            "[workspace.dependencies]\nbar = { version = \"1.1.0\", path = \"packages/bar\" }\n",
        );
        let changes = inherited_changes(&keys, &old, &new);
        assert_eq!(
            changes,
            vec![InheritedChange {
                field: "workspace.dependencies.bar.version".to_string(),
            }]
        );
    }

    #[test]
    fn formatting_only_workspace_value_is_not_a_change() {
        let keys = InheritedKeys {
            package: vec!["edition".to_string()],
            dependencies: vec![],
        };
        let old = doc("[workspace.package]\nedition = \"2024\"\n");
        let new = doc("[workspace.package]\n  edition = \"2024\"\n");
        assert!(inherited_changes(&keys, &old, &new).is_empty());
    }
}
