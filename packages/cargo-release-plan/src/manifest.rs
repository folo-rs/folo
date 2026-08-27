// Manifest parsing for versions, packaging rules, members, and pins.

use std::fmt;
use std::path::Path;

use ignore::overrides::{Override, OverrideBuilder};
use ohno::AppError;
use semver::Version;
use toml_edit::{DocumentMut, Item, TableLike, Value};

use crate::inherited::{InheritedKeys, collect_inherited_keys, is_workspace_inherit};
use crate::packaging::PackagingRules;
use crate::{InvalidMemberPatternError, InvalidVersionError, ParseTomlError};

/// Parsed facts about one package manifest.
#[derive(Clone, Debug)]
pub(crate) struct PackageManifest {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) directory: String,
    pub(crate) packaging: PackagingRules,
    pub(crate) inherited: InheritedKeys,
    pub(crate) publish: bool,
}

/// Workspace member patterns from the root manifest, compiled for repeated queries.
///
/// A historical snapshot tests every discovered manifest against the same
/// patterns, so the matchers are built once when the root manifest is parsed
/// rather than per candidate directory.
#[derive(Clone, Debug, Default)]
pub(crate) struct WorkspaceMembers {
    members: Vec<MemberPattern>,
    exclude: Vec<MemberPattern>,
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
    let version = if is_workspace_inherit(version_item) {
        let Some(version) = workspace_version else {
            return Ok(None);
        };
        version
    } else {
        let Some(version) = version_item.as_str() else {
            return Ok(None);
        };
        version
    };
    let version = version
        .parse::<Version>()
        .map_err(|error| InvalidVersionError::caused_by(name, version, error))?;
    let directory = directory_of(manifest_path);
    Ok(Some(PackageManifest {
        name: name.to_string(),
        version,
        directory,
        packaging: packaging_from_package(package)?,
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
        members: compile_patterns(&string_array(workspace.get("members")))?,
        exclude: compile_patterns(&string_array(workspace.get("exclude")))?,
    })
}

/// Whether `dir` (repo-relative, `/` separators) matches a workspace member pattern.
pub(crate) fn is_workspace_member(dir: &str, members: &WorkspaceMembers) -> bool {
    let dir = dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    if members.exclude.iter().any(|pattern| pattern.matches(dir)) {
        return false;
    }
    if members.members.is_empty() {
        // A manifest without a `members` list defines a workspace whose only
        // member is the root package itself. Treating an absent list as "every
        // manifest in the repository" would pull unrelated packages into a
        // historical snapshot.
        return dir.is_empty();
    }
    members.members.iter().any(|pattern| pattern.matches(dir))
}

fn compile_patterns(patterns: &[String]) -> Result<Vec<MemberPattern>, AppError> {
    patterns
        .iter()
        .map(|pattern| MemberPattern::new(pattern))
        .collect()
}

/// One compiled `[workspace] members` / `exclude` pattern.
struct MemberPattern {
    literal: String,
    matcher: Override,
}

impl MemberPattern {
    fn new(pattern: &str) -> Result<Self, AppError> {
        let literal = pattern.replace('\\', "/");
        let mut matcher = OverrideBuilder::new("");
        matcher
            .add(&literal)
            .map_err(|error| InvalidMemberPatternError::caused_by(&literal, error))?;
        let matcher = matcher
            .build()
            .map_err(|error| InvalidMemberPatternError::caused_by(&literal, error))?;
        Ok(Self { literal, matcher })
    }

    fn matches(&self, dir: &str) -> bool {
        if self.literal == dir {
            return true;
        }
        // `foo/**` in Cargo member lists includes the `foo` directory itself.
        if let Some(prefix) = self.literal.strip_suffix("/**")
            && dir == prefix
        {
            return true;
        }
        self.matcher.matched(dir, true).is_whitelist()
    }
}

// `Override` has no `Clone`, and the compiled matchers are immutable after
// construction, so cloning a member set recompiles its patterns. Patterns that
// compiled once compile again, so the fallback is unreachable in practice.
impl Clone for MemberPattern {
    fn clone(&self) -> Self {
        Self::new(&self.literal).unwrap_or_else(|_| Self {
            literal: self.literal.clone(),
            matcher: Override::empty(),
        })
    }
}

impl fmt::Debug for MemberPattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MemberPattern")
            .field("literal", &self.literal)
            .finish_non_exhaustive()
    }
}

fn packaging_from_package(package: &dyn TableLike) -> Result<PackagingRules, AppError> {
    let include = opt_string_array(package.get("include"));
    let exclude = opt_string_array(package.get("exclude"));
    PackagingRules::new(include.as_deref(), exclude.as_deref())
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

    fn members(patterns: &[&str]) -> WorkspaceMembers {
        WorkspaceMembers {
            members: compile_patterns(
                &patterns
                    .iter()
                    .map(|pattern| (*pattern).to_string())
                    .collect::<Vec<_>>(),
            )
            .unwrap(),
            exclude: Vec::new(),
        }
    }

    #[test]
    fn member_pattern_matches_one_segment_star() {
        let packages = members(&["packages/*"]);
        assert!(is_workspace_member("packages/foo", &packages));
        assert!(!is_workspace_member("packages/foo/bar", &packages));
        assert!(!is_workspace_member("other/foo", &packages));
        let crates = members(&["crates/foo-*"]);
        assert!(is_workspace_member("crates/foo-bar", &crates));
        assert!(!is_workspace_member("crates/foo-bar/nested", &crates));
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
    fn member_pattern_double_star_matches_the_prefix_directory() {
        let packages = members(&["packages/**"]);
        assert!(is_workspace_member("packages", &packages));
        assert!(is_workspace_member("packages/foo", &packages));
        assert!(!is_workspace_member("other", &packages));
    }

    #[test]
    fn invalid_member_pattern_is_rejected() {
        // An unclosed brace is not a valid path pattern; a silently non-matching
        // pattern would drop real members from a historical snapshot.
        let error = parse_workspace_members(
            "[workspace]\nmembers = [\"foo.{js,ts\"]\n",
            Path::new("Cargo.toml"),
        )
        .unwrap_err();
        assert_eq!(
            error
                .find_source::<InvalidMemberPatternError>()
                .map(InvalidMemberPatternError::pattern),
            Some("foo.{js,ts")
        );
    }

    #[test]
    fn workspace_members_exclude_and_empty_members() {
        let declared = parse_workspace_members(
            r#"
[workspace]
members = ["packages/*"]
exclude = ["packages/skip"]
"#,
            Path::new("Cargo.toml"),
        )
        .unwrap();
        assert!(is_workspace_member("packages/foo", &declared));
        assert!(!is_workspace_member("packages/skip", &declared));
        assert!(!is_workspace_member("examples/foo", &declared));
        // Without a `members` list the only member is the root package, matching
        // Cargo rather than treating every manifest in the tree as a member.
        let empty = parse_workspace_members("[workspace]\n", Path::new("Cargo.toml")).unwrap();
        assert!(is_workspace_member("", &empty));
        assert!(!is_workspace_member("anything", &empty));
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
