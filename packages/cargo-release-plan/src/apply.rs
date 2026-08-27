// `apply` command: rewrite versions, expand groups, refresh the lockfile.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use ohno::AppError;
use semver::{Version, VersionReq};
use toml_edit::{DocumentMut, Formatted, Item, TableLike, Value};

use crate::command::run_capture;
use crate::inherited::is_workspace_inherit;
use crate::manifest::parse_document;
use crate::metadata::{WorkTree, load_work_tree};
use crate::plan::{ExpandedPlan, PlanFile, expand_plan};
use crate::text::plural;
use crate::verbose::Verbose;
use crate::{ParsePlanError, ReadFileError, WriteFileError};

// Cargo's package-level dependency tables, plus the same names under
// `[target.<spec>]`. These are the only places intra-workspace requirements live.
const PACKAGE_DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// One on-disk manifest after an in-memory rewrite, waiting to be written.
struct ManifestEdit {
    path: PathBuf,
    original: String,
    updated: String,
}

pub(crate) fn run_apply(
    plan_path: &Path,
    dry_run: bool,
    manifest_path: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let plan = fs::read_to_string(plan_path)
        .map_err(|error| ReadFileError::caused_by(plan_path, error))?;
    let plan: PlanFile =
        serde_json::from_str(&plan).map_err(|error| ParsePlanError::caused_by(plan_path, error))?;

    let work_tree = load_work_tree(manifest_path)?;
    let current: BTreeMap<String, Version> = work_tree
        .packages
        .iter()
        .map(|package| {
            (
                package.manifest.name.clone(),
                package.manifest.version.clone(),
            )
        })
        .collect();
    let expanded = expand_plan(&plan, &work_tree.groups, &current, &current)?;
    verbose.note(format!(
        "plan expands to {}; group members are included even when the plan named only one of \
         them, and never-published members on this branch are included in apply",
        plural(expanded.packages.len(), "package version")
    ));

    let edits = compute_edits(&work_tree, &expanded, verbose)?;
    let changed = changed_edit_count(&edits);

    if dry_run {
        return Ok(dry_run_summary(&edits, expanded.packages.len()));
    }

    for edit in &edits {
        if edit.original == edit.updated {
            continue;
        }
        fs::write(&edit.path, edit.updated.as_bytes())
            .map_err(|error| WriteFileError::caused_by(&edit.path, error))?;
        verbose.note(format!(
            "wrote {} after computing the full edit set in memory; remaining writes can still fail",
            edit.path.display()
        ));
    }

    let refreshed = refresh_lockfile(&work_tree, &expanded, verbose)?;

    let lockfile = if refreshed {
        "refreshed the workspace lockfile"
    } else {
        "left the workspace lockfile untouched"
    };
    Ok(format!(
        "Updated {} and {lockfile}",
        plural(changed, "manifest")
    ))
}

fn compute_edits(
    work_tree: &WorkTree,
    expanded: &ExpandedPlan,
    verbose: Verbose,
) -> Result<Vec<ManifestEdit>, AppError> {
    let mut edits = Vec::new();
    let root = work_tree.workspace_root.join("Cargo.toml");
    edits.push(edit_path(&root, |doc| {
        rewrite_workspace_dependencies(doc, expanded, verbose);
        rewrite_package_version(doc, expanded, verbose);
        rewrite_dependency_tables(doc, expanded, verbose);
    })?);

    let mut seen = HashSet::new();
    seen.insert(root);
    // Every member is rewritten, not just the publishable ones: a `publish = false`
    // member can still carry an `=` pin on a package the plan increments, and
    // leaving it stale would break the workspace lockfile refresh below.
    for manifest_path in &work_tree.member_manifests {
        if !seen.insert(manifest_path.clone()) {
            continue;
        }
        edits.push(edit_path(manifest_path, |doc| {
            rewrite_package_version(doc, expanded, verbose);
            rewrite_dependency_tables(doc, expanded, verbose);
        })?);
    }
    Ok(edits)
}

fn dry_run_summary(edits: &[ManifestEdit], package_count: usize) -> String {
    let changed = changed_edit_count(edits);
    let mut message = format!("Dry run: {} would change", plural(changed, "manifest"));
    for edit in edits {
        if edit.original != edit.updated {
            write!(message, "\n  {}", edit.path.display()).expect("writing to String");
        }
    }
    write!(
        message,
        "; lockfile would be refreshed for {}",
        plural(package_count, "package")
    )
    .expect("writing to String");
    message
}

fn changed_edit_count(edits: &[ManifestEdit]) -> usize {
    edits
        .iter()
        .filter(|edit| edit.original != edit.updated)
        .count()
}

fn edit_path(
    path: &Path,
    rewrite: impl FnOnce(&mut DocumentMut),
) -> Result<ManifestEdit, AppError> {
    let original =
        fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    let mut doc: DocumentMut = parse_document(path, &original)?;
    rewrite(&mut doc);
    Ok(ManifestEdit {
        path: path.to_path_buf(),
        original,
        updated: doc.to_string(),
    })
}

fn rewrite_package_version(doc: &mut DocumentMut, expanded: &ExpandedPlan, verbose: Verbose) {
    let Some(package) = doc.get("package").and_then(Item::as_table_like) else {
        return;
    };
    let Some(name) = package.get("name").and_then(Item::as_str) else {
        return;
    };
    let name = name.to_string();
    let Some(new_version) = expanded.packages.get(&name) else {
        return;
    };
    let Some(package) = doc.get_mut("package").and_then(Item::as_table_like_mut) else {
        return;
    };
    if set_package_version_item(package, new_version) {
        verbose.note(format!(
            "{name}: set package.version to {new_version} because the expanded plan assigns that \
             version to this package"
        ));
    }
}

fn rewrite_workspace_dependencies(
    doc: &mut DocumentMut,
    expanded: &ExpandedPlan,
    verbose: Verbose,
) {
    let Some(workspace) = doc.get_mut("workspace").and_then(Item::as_table_like_mut) else {
        return;
    };
    let Some(deps) = workspace
        .get_mut("dependencies")
        .and_then(Item::as_table_like_mut)
    else {
        return;
    };
    rewrite_dep_table(deps, expanded, verbose, "workspace.dependencies");
}

// Walks every dependency table; entry-level rewrite is tested separately.
#[cfg_attr(test, mutants::skip)]
fn rewrite_dependency_tables(doc: &mut DocumentMut, expanded: &ExpandedPlan, verbose: Verbose) {
    for table_name in PACKAGE_DEP_TABLES {
        if let Some(table) = doc.get_mut(table_name).and_then(Item::as_table_like_mut) {
            rewrite_dep_table(table, expanded, verbose, table_name);
        }
    }

    let specs: Vec<String> = match doc.get("target").and_then(Item::as_table_like) {
        Some(target) => target.iter().map(|(key, _)| key.to_string()).collect(),
        None => Vec::new(),
    };
    for spec in specs {
        for table_name in PACKAGE_DEP_TABLES {
            let Some(table) = doc
                .get_mut("target")
                .and_then(Item::as_table_like_mut)
                .and_then(|target| target.get_mut(spec.as_str()))
                .and_then(Item::as_table_like_mut)
                .and_then(|spec_table| spec_table.get_mut(table_name))
                .and_then(Item::as_table_like_mut)
            else {
                continue;
            };
            rewrite_dep_table(
                table,
                expanded,
                verbose,
                &format!("target.{spec}.{table_name}"),
            );
        }
    }
}

fn rewrite_dep_table(
    table: &mut dyn TableLike,
    expanded: &ExpandedPlan,
    verbose: Verbose,
    where_: &str,
) {
    for (key, entry) in table.iter_mut() {
        let name = key.get().to_string();
        if !dep_has_path(entry) {
            continue;
        }
        let crate_name = dep_crate_name(entry, &name);
        let Some(new_version) = expanded.packages.get(&crate_name).cloned() else {
            continue;
        };
        if rewrite_dep_entry(entry, &new_version) {
            verbose.note(format!(
                "{where_}.{name}: rewrote the version requirement to follow {crate_name} {new_version} \
                 (exact `=` pins keep the equals sign; requirements that already match the new \
                 version are left unchanged; only path dependencies are rewritten)"
            ));
        }
    }
}

fn dep_has_path(entry: &Item) -> bool {
    match entry {
        Item::Value(Value::InlineTable(table)) => table.contains_key("path"),
        Item::Table(table) => table.contains_key("path"),
        _ => false,
    }
}

fn dep_crate_name(entry: &Item, table_key: &str) -> String {
    let package = match entry {
        Item::Value(Value::InlineTable(table)) => table
            .get("package")
            .and_then(Value::as_str)
            .map(str::to_owned),
        Item::Table(table) => table
            .get("package")
            .and_then(Item::as_str)
            .map(str::to_owned),
        _ => None,
    };
    package.unwrap_or_else(|| table_key.to_string())
}

fn rewrite_dep_entry(entry: &mut Item, new_version: &Version) -> bool {
    match entry {
        Item::Value(Value::String(formatted)) => {
            let rewritten = rewrite_req(formatted.value(), new_version);
            set_formatted(formatted, rewritten)
        }
        Item::Value(Value::InlineTable(table)) => {
            if let Some(Value::String(formatted)) = table.get_mut("version") {
                let rewritten = rewrite_req(formatted.value(), new_version);
                set_formatted(formatted, rewritten)
            } else {
                false
            }
        }
        Item::Table(table) => set_version_item(table, "version", new_version),
        _ => false,
    }
}

fn set_package_version_item(table: &mut dyn TableLike, new_version: &Version) -> bool {
    match table.get_mut("version") {
        Some(Item::Value(Value::String(formatted))) => {
            set_formatted(formatted, new_version.to_string())
        }
        Some(item) if is_workspace_inherit(item) => {
            *item = Item::Value(Value::from(new_version.to_string()));
            true
        }
        _ => false,
    }
}

fn set_version_item(table: &mut dyn TableLike, key: &str, new_version: &Version) -> bool {
    match table.get_mut(key) {
        Some(Item::Value(Value::String(formatted))) => {
            let rewritten = rewrite_req(formatted.value(), new_version);
            set_formatted(formatted, rewritten)
        }
        _ => false,
    }
}

fn set_formatted(formatted: &mut Formatted<String>, rewritten: String) -> bool {
    if rewritten == formatted.value().as_str() {
        return false;
    }
    let decor = formatted.decor().clone();
    let mut replacement = Formatted::new(rewritten);
    *replacement.decor_mut() = decor;
    *formatted = replacement;
    true
}

fn rewrite_req(old: &str, new_version: &Version) -> String {
    let trimmed = old.trim();
    if trimmed.starts_with('=') {
        return format!("={new_version}");
    }
    if let Ok(req) = VersionReq::parse(trimmed)
        && req.matches(new_version)
    {
        return old.to_string();
    }
    new_version.to_string()
}

// Spawns `cargo update --offline`; lockfile is not released content.
#[cfg_attr(test, mutants::skip)]
fn refresh_lockfile(
    work_tree: &WorkTree,
    expanded: &ExpandedPlan,
    verbose: Verbose,
) -> Result<bool, AppError> {
    let lockfile = work_tree.workspace_root.join("Cargo.lock");
    if expanded.packages.is_empty() {
        verbose.note(
            "plan expands to no packages, so apply skips the lockfile refresh rather than \
             running a workspace-wide cargo update",
        );
        return Ok(false);
    }
    if !lockfile.exists() {
        verbose.note(
            "no Cargo.lock present, so apply skips the lockfile refresh; the lockfile is not \
             released content",
        );
        return Ok(false);
    }
    let mut args = vec![
        "update".to_string(),
        "--offline".to_string(),
        "--manifest-path".to_string(),
        work_tree
            .workspace_root
            .join("Cargo.toml")
            .to_string_lossy()
            .into_owned(),
    ];
    for name in expanded.packages.keys() {
        args.push("-p".to_string());
        args.push(name.clone());
    }
    let args_ref: Vec<&str> = args.iter().map(String::as_str).collect();
    verbose.note(format!(
        "refreshing the workspace lockfile with `cargo {}` so --locked builds observe the new \
         path-dependency versions; the lockfile is not released content and cannot re-trigger check",
        args_ref.join(" ")
    ));
    _ = run_capture("cargo", &args_ref, &work_tree.workspace_root)?;
    Ok(true)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn dep_item(toml: &str) -> DocumentMut {
        toml.parse::<DocumentMut>().unwrap()
    }

    fn first_dep(doc: &mut DocumentMut) -> &mut Item {
        doc.get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .and_then(|table| table.get_mut("foo"))
            .unwrap()
    }

    #[test]
    fn changed_edit_count_counts_only_rewritten_manifests() {
        let edits = [
            ManifestEdit {
                path: PathBuf::from("unchanged.toml"),
                original: "a".to_string(),
                updated: "a".to_string(),
            },
            ManifestEdit {
                path: PathBuf::from("changed.toml"),
                original: "a".to_string(),
                updated: "b".to_string(),
            },
        ];
        assert_eq!(changed_edit_count(&edits), 1);
        assert_eq!(changed_edit_count(&edits[..1]), 0);
        let summary = dry_run_summary(&edits, 2);
        assert!(summary.contains("changed.toml"));
        assert!(!summary.contains("unchanged.toml"));
    }

    #[test]
    fn rewrite_req_keeps_matching_caret_and_rewrites_equals() {
        let new = v("0.1.1");
        assert_eq!(rewrite_req("0.1.0", &new), "0.1.0");
        assert_eq!(rewrite_req("^0.1", &new), "^0.1");
        assert_eq!(rewrite_req("=0.1.0", &new), "=0.1.1");
        assert_eq!(rewrite_req("0.1.0", &v("0.2.0")), "0.2.0");
    }

    #[test]
    fn set_version_item_replaces_workspace_inherit() {
        let mut doc = dep_item(
            r#"
[package]
name = "foo"
version.workspace = true
"#,
        );
        let package = doc
            .get_mut("package")
            .and_then(Item::as_table_like_mut)
            .unwrap();
        assert!(set_package_version_item(package, &v("0.2.0")));
        assert!(doc.to_string().contains("version = \"0.2.0\""));
        assert!(!doc.to_string().contains("workspace"));
    }

    #[test]
    fn dep_has_path_detects_path_tables() {
        let doc = dep_item("[dependencies]\nfoo = { version = \"0.1.0\", path = \"../foo\" }\n");
        let entry = doc
            .get("dependencies")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("foo"))
            .unwrap();
        assert!(dep_has_path(entry));
        let doc = dep_item("[dependencies]\nfoo = \"0.1.0\"\n");
        let entry = doc
            .get("dependencies")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("foo"))
            .unwrap();
        assert!(!dep_has_path(entry));
    }

    #[test]
    fn rewrite_dep_entry_updates_bare_string_and_table() {
        let mut doc = dep_item("[dependencies]\nfoo = \"0.1.0\"\n");
        assert!(rewrite_dep_entry(first_dep(&mut doc), &v("0.2.0")));
        assert!(doc.to_string().contains("0.2.0"));

        let mut doc = dep_item("[dependencies]\nfoo = \"=0.1.0\"\n");
        assert!(rewrite_dep_entry(first_dep(&mut doc), &v("0.1.1")));
        assert!(doc.to_string().contains("=0.1.1"));

        let mut doc = dep_item("[dependencies]\nfoo = { version = \"0.1.0\", path = \"../x\" }\n");
        assert!(rewrite_dep_entry(first_dep(&mut doc), &v("0.2.0")));
        assert!(doc.to_string().contains("version = \"0.2.0\""));

        let mut doc = dep_item(
            "
[dependencies.foo]
version = \"0.1.0\"
",
        );
        assert!(rewrite_dep_entry(first_dep(&mut doc), &v("0.3.0")));
        assert!(doc.to_string().contains("0.3.0"));
    }

    #[test]
    fn dep_crate_name_reads_package_alias() {
        let doc =
            dep_item("[dependencies]\nfoo-alias = { package = \"foo\", version = \"0.1.0\" }\n");
        let entry = doc
            .get("dependencies")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("foo-alias"))
            .unwrap();
        assert_eq!(dep_crate_name(entry, "foo-alias"), "foo");
        let doc = dep_item("[dependencies]\nfoo = \"0.1.0\"\n");
        let entry = doc
            .get("dependencies")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("foo"))
            .unwrap();
        assert_eq!(dep_crate_name(entry, "foo"), "foo");
        let doc = dep_item(
            "
[dependencies.foo-alias]
package = \"foo\"
version = \"0.1.0\"
",
        );
        let entry = doc
            .get("dependencies")
            .and_then(Item::as_table_like)
            .and_then(|table| table.get("foo-alias"))
            .unwrap();
        assert_eq!(dep_crate_name(entry, "foo-alias"), "foo");
    }
    /// Cargo accepts a dependency as a bare requirement string, an inline
    /// table, or a table of its own, and a `=` pin in any of them has to follow
    /// the package it pins.
    #[test]
    fn every_dependency_declaration_form_is_rewritten() {
        let new = v("0.2.0");

        let mut bare = Item::Value(Value::from("=0.1.0"));
        assert!(rewrite_dep_entry(&mut bare, &new));
        assert_eq!(bare.as_str(), Some("=0.2.0"));

        let mut doc = dep_item("[dependencies.demo]\nversion = \"=0.1.0\"\npath = \"../demo\"\n");
        let entry = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .and_then(|deps| deps.get_mut("demo"))
            .unwrap();
        assert!(rewrite_dep_entry(entry, &new));
        assert!(doc.to_string().contains("version = \"=0.2.0\""), "{doc}");
    }

    /// A path dependency can omit the version entirely, which leaves nothing to
    /// rewrite rather than being an error.
    #[test]
    fn a_dependency_without_a_version_is_left_alone() {
        let new = v("0.2.0");

        let mut doc = dep_item("[dependencies.demo]\npath = \"../demo\"\n");
        let entry = doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .and_then(|deps| deps.get_mut("demo"))
            .unwrap();
        assert!(!rewrite_dep_entry(entry, &new));

        let mut inline_doc = dep_item("[dependencies]\ndemo = { path = \"../demo\" }\n");
        let inline = inline_doc
            .get_mut("dependencies")
            .and_then(Item::as_table_like_mut)
            .and_then(|deps| deps.get_mut("demo"))
            .unwrap();
        assert!(!rewrite_dep_entry(inline, &new));
    }

    #[test]
    fn a_requirement_that_already_matches_is_not_rewritten() {
        let mut bare = Item::Value(Value::from("=0.2.0"));
        assert!(!rewrite_dep_entry(&mut bare, &v("0.2.0")));
    }
}
