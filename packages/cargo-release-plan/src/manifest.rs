// Manifest parsing for versions, packaging rules, members, and pins.

use std::path::Path;

use ignore::overrides::OverrideBuilder;
use ohno::AppError;
use semver::Version;
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::inherited::{InheritedKeys, collect_inherited_keys};
use crate::packaging::PackagingRules;
use crate::{InvalidVersionError, ParseTomlError};

/// Parsed facts about one package manifest.
#[derive(Clone, Debug)]
pub(crate) struct PackageManifest {
    pub name: String,
    pub version: Version,
    pub directory: String,
    pub packaging: PackagingRules,
    pub inherited: InheritedKeys,
    pub publish: bool,
}

/// Workspace member glob patterns from the root manifest.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkspaceMembers {
    pub members: Vec<String>,
    pub exclude: Vec<String>,
}

pub(crate) fn parse_document(path: &Path, content: &str) -> Result<DocumentMut, AppError> {
    content
        .parse()
        .map_err(|error| ParseTomlError::caused_by(path, error).into())
}

pub(crate) fn parse_package_manifest(
    content: &str,
    manifest_path: &str,
    workspace_version: Option<&str>,
) -> Result<Option<PackageManifest>, AppError> {
    let path = Path::new(manifest_path);
    let doc = parse_document(path, content)?;
    let Some(package) = doc.get("package").and_then(Item::as_table_like) else {
        return Ok(None);
    };
    let Some(name) = package.get("name").and_then(Item::as_str) else {
        return Ok(None);
    };
    let Some(version_item) = package.get("version") else {
        return Ok(None);
    };
    let version_str = if crate::inherited::is_workspace_inherit(version_item) {
        let Some(version_str) = workspace_version else {
            return Ok(None);
        };
        version_str
    } else {
        let Some(version_str) = version_item.as_str() else {
            return Ok(None);
        };
        version_str
    };
    let version = version_str
        .parse::<Version>()
        .map_err(|error| InvalidVersionError::caused_by(name, version_str, error))?;
    let directory = directory_of(manifest_path);
    Ok(Some(PackageManifest {
        name: name.to_string(),
        version,
        directory,
        packaging: packaging_from_package(package),
        inherited: collect_inherited_keys(&doc),
        publish: publish_allowed(package),
    }))
}

pub(crate) fn parse_workspace_members(
    content: &str,
    path: &Path,
) -> Result<WorkspaceMembers, AppError> {
    let doc = parse_document(path, content)?;
    let Some(workspace) = doc.get("workspace").and_then(Item::as_table_like) else {
        return Ok(WorkspaceMembers::default());
    };
    Ok(WorkspaceMembers {
        members: string_array(workspace.get("members")),
        exclude: string_array(workspace.get("exclude")),
    })
}

/// Whether `dir` (repo-relative, `/` separators) matches a workspace member glob.
pub(crate) fn is_workspace_member(dir: &str, members: &WorkspaceMembers) -> bool {
    let dir = dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    if members
        .exclude
        .iter()
        .any(|pattern| glob_member(pattern, dir))
    {
        return false;
    }
    if members.members.is_empty() {
        return true;
    }
    members
        .members
        .iter()
        .any(|pattern| glob_member(pattern, dir))
}

fn glob_member(pattern: &str, dir: &str) -> bool {
    let pattern = pattern.replace('\\', "/");
    let dir = dir.replace('\\', "/");
    if pattern == dir {
        return true;
    }
    // `foo/**` in Cargo member lists includes the `foo` directory itself.
    if let Some(prefix) = pattern.strip_suffix("/**")
        && dir == prefix
    {
        return true;
    }
    let mut builder = OverrideBuilder::new("");
    if builder.add(&pattern).is_err() {
        return false;
    }
    let Ok(over) = builder.build() else {
        return false;
    };
    over.matched(dir, true).is_whitelist()
}

fn packaging_from_package(package: &dyn TableLike) -> PackagingRules {
    PackagingRules::new(
        opt_string_array(package.get("include")),
        opt_string_array(package.get("exclude")),
    )
}

fn publish_allowed(package: &dyn TableLike) -> bool {
    match package.get("publish") {
        None => true,
        Some(item) => match item {
            Item::Value(Value::Boolean(b)) => *b.value(),
            Item::Value(Value::Array(array)) => !array.is_empty(),
            _ => true,
        },
    }
}

fn string_array(item: Option<&Item>) -> Vec<String> {
    opt_string_array(item).unwrap_or_default()
}

fn opt_string_array(item: Option<&Item>) -> Option<Vec<String>> {
    let item = item?;
    let array = item.as_array()?;
    Some(
        array
            .iter()
            .filter_map(Value::as_str)
            .map(ToOwned::to_owned)
            .collect(),
    )
}

fn directory_of(manifest_path: &str) -> String {
    let path = manifest_path.replace('\\', "/");
    match path.rsplit_once('/') {
        Some((dir, _)) => dir.to_string(),
        None => String::new(),
    }
}

/// Repo-relative directory of a work-tree manifest path.
pub(crate) fn repo_relative_dir(workspace_root: &Path, manifest_path: &Path) -> String {
    let parent = manifest_path.parent().unwrap_or(manifest_path);
    parent
        .strip_prefix(workspace_root)
        .unwrap_or(parent)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn glob_member_matches_one_segment_star() {
        assert!(glob_member("packages/*", "packages/foo"));
        assert!(!glob_member("packages/*", "packages/foo/bar"));
        assert!(!glob_member("packages/*", "other/foo"));
        assert!(glob_member("crates/foo-*", "crates/foo-bar"));
        assert!(!glob_member("crates/foo-*", "crates/foo-bar/nested"));
    }

    #[test]
    fn publish_false_excludes_package() {
        let parsed = parse_package_manifest(
            r#"
[package]
name = "priv"
version = "0.1.0"
publish = false
"#,
            "packages/priv/Cargo.toml",
            None,
        )
        .unwrap()
        .unwrap();
        assert!(!parsed.publish);
    }

    #[test]
    fn glob_member_double_star_matches_the_prefix_directory() {
        assert!(glob_member("packages/**", "packages"));
        assert!(glob_member("packages/**", "packages/foo"));
        assert!(!glob_member("packages/**", "other"));
    }

    #[test]
    fn workspace_members_exclude_and_empty_members() {
        let members = parse_workspace_members(
            r#"
[workspace]
members = ["packages/*"]
exclude = ["packages/skip"]
"#,
            Path::new("Cargo.toml"),
        )
        .unwrap();
        assert!(is_workspace_member("packages/foo", &members));
        assert!(!is_workspace_member("packages/skip", &members));
        assert!(!is_workspace_member("examples/foo", &members));
        let empty = parse_workspace_members("[workspace]\n", Path::new("Cargo.toml")).unwrap();
        assert!(empty.members.is_empty());
        assert!(is_workspace_member("anything", &empty));
    }

    #[test]
    fn publish_registry_array_is_allowed_when_nonempty() {
        let allowed = parse_package_manifest(
            r#"
[package]
name = "pub"
version = "0.1.0"
publish = ["crates-io"]
"#,
            "packages/pub/Cargo.toml",
            None,
        )
        .unwrap()
        .unwrap();
        assert!(allowed.publish);
        let empty = parse_package_manifest(
            r#"
[package]
name = "nopub"
version = "0.1.0"
publish = []
"#,
            "packages/nopub/Cargo.toml",
            None,
        )
        .unwrap()
        .unwrap();
        assert!(!empty.publish);
    }

    #[test]
    fn include_array_becomes_packaging_rules() {
        let parsed = parse_package_manifest(
            r#"
[package]
name = "foo"
version = "0.1.0"
include = ["src/**", "README.md"]
"#,
            "packages/foo/Cargo.toml",
            None,
        )
        .unwrap()
        .unwrap();
        assert!(parsed.packaging.is_released("src/lib.rs"));
        assert!(!parsed.packaging.is_released("tests/x.rs"));
    }

    #[test]
    fn inherited_workspace_version_is_parsed() {
        let parsed = parse_package_manifest(
            r#"
[package]
name = "foo"
version.workspace = true
"#,
            "packages/foo/Cargo.toml",
            Some("0.4.0"),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.version.to_string(), "0.4.0");
        assert!(
            parse_package_manifest(
                r#"
[package]
name = "foo"
version.workspace = true
"#,
                "packages/foo/Cargo.toml",
                None,
            )
            .unwrap()
            .is_none()
        );
    }
}
