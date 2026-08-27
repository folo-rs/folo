// Manifest parsing for versions, packaging rules, members, and pins.

use std::collections::HashSet;
use std::path::Path;
use std::{fmt, fs};

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
    /// Dependency paths declared by this package, relative to its own directory.
    pub(crate) path_dependencies: Vec<String>,
    /// Dependency paths this package inherits, relative to the workspace root.
    ///
    /// `[workspace.dependencies]` declares its paths relative to the workspace
    /// root, so these cannot be joined onto the member directory the way a
    /// locally declared path is.
    pub(crate) inherited_path_dependencies: Vec<String>,
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

/// How the filesystem hosting a workspace resolves path case.
///
/// Cargo opens member directories through the filesystem while Git reports the
/// spelling recorded in the tree, so member matching only agrees with Cargo when
/// it applies the same case rules. Case sensitivity is a property of the volume
/// and directory rather than of the operating system, so it is probed.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum PathCase {
    /// Two spellings that differ in case name different paths.
    #[default]
    Sensitive,
    /// Two spellings that differ only in case name the same path.
    Insensitive,
}

impl PathCase {
    /// Probes how the filesystem holding `dir` resolves path case.
    ///
    /// The probe re-opens an existing entry under a case-flipped spelling, so it
    /// writes nothing and works on a read-only checkout. A directory that cannot
    /// be read, or that holds no entry whose flipped spelling is unambiguous,
    /// yields the stricter answer, which never widens member matching.
    pub(crate) fn probe(dir: &Path) -> Self {
        let Ok(entries) = fs::read_dir(dir) else {
            return Self::Sensitive;
        };
        let names: Vec<String> = entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let present: HashSet<&str> = names.iter().map(String::as_str).collect();
        for name in &names {
            let flipped = flip_case(name);
            // An entry that is already present under both spellings proves
            // nothing, and a name without cased characters cannot be flipped.
            if flipped == *name || present.contains(flipped.as_str()) {
                continue;
            }
            return if dir.join(&flipped).exists() {
                Self::Insensitive
            } else {
                Self::Sensitive
            };
        }
        Self::Sensitive
    }

    /// Whether two path components name the same path under these case rules.
    fn same_path(self, left: &str, right: &str) -> bool {
        match self {
            Self::Sensitive => left == right,
            Self::Insensitive => left.to_lowercase() == right.to_lowercase(),
        }
    }
}

fn flip_case(name: &str) -> String {
    name.chars()
        .flat_map(|c| {
            if c.is_uppercase() {
                c.to_lowercase().collect::<Vec<_>>()
            } else {
                c.to_uppercase().collect()
            }
        })
        .collect()
}

pub(crate) fn parse_document(path: &Path, content: &str) -> Result<DocumentMut, AppError> {
    content
        .parse()
        .map_err(|error| ParseTomlError::caused_by(path, error).into())
}

pub(crate) fn parse_package_manifest(
    content: &str,
    manifest_path: &str,
    workspace: &WorkspaceInherit<'_>,
) -> Result<Option<PackageManifest>, AppError> {
    let path = Path::new(manifest_path);
    let doc = parse_document(path, content)?;
    // A manifest without a complete `[package]` identity is not something Cargo
    // would publish: it is either a virtual workspace root or a member whose
    // version is inherited from a root that does not declare one. Both are
    // ordinary states of historical trees, so they yield "not a package" rather
    // than an error. Malformed values that Cargo would reject still error below.
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
        let Some(version) = workspace.package_version() else {
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
        packaging: packaging_from_package(package, workspace)?,
        inherited: collect_inherited_keys(&doc),
        publish: publish_allowed(package, workspace),
        path_dependencies: path_dependencies(&doc),
        inherited_path_dependencies: inherited_path_dependencies(&doc, workspace),
    }))
}

/// The `[workspace.package]` and `[workspace.dependencies]` tables a member inherits from.
///
/// Historical snapshots parse member manifests without Cargo's help, so every
/// `.workspace = true` key has to be resolved against the root manifest of the
/// same commit or the member would be read with Cargo's defaults instead of the
/// values it actually declares.
#[derive(Clone, Copy, Default)]
pub(crate) struct WorkspaceInherit<'a> {
    package: Option<&'a dyn TableLike>,
    dependencies: Option<&'a dyn TableLike>,
}

impl<'a> WorkspaceInherit<'a> {
    pub(crate) fn from_root(root: &'a DocumentMut) -> Self {
        let workspace = root.get("workspace").and_then(Item::as_table_like);
        Self {
            package: workspace
                .and_then(|workspace| workspace.get("package"))
                .and_then(Item::as_table_like),
            dependencies: workspace
                .and_then(|workspace| workspace.get("dependencies"))
                .and_then(Item::as_table_like),
        }
    }

    fn package_version(&self) -> Option<&'a str> {
        self.package_key("version").and_then(Item::as_str)
    }

    fn package_key(&self, key: &str) -> Option<&'a Item> {
        let package = self.package?;
        package.get(key)
    }

    fn dependency(&self, name: &str) -> Option<&'a dyn TableLike> {
        let dependencies = self.dependencies?;
        dependencies.get(name).and_then(Item::as_table_like)
    }
}

/// Collects every `path` a package reaches through `[workspace.dependencies]`.
///
/// Cargo makes an inherited path dependency a member exactly as it does a
/// locally declared one, so historical membership only matches Cargo once these
/// edges are followed as well.
fn inherited_path_dependencies(doc: &DocumentMut, workspace: &WorkspaceInherit<'_>) -> Vec<String> {
    let mut names = Vec::new();
    collect_inherited_dependency_names(doc.as_table(), &mut names, 0);
    names
        .iter()
        .filter_map(|name| {
            workspace
                .dependency(name)
                .and_then(|dependency| dependency.get("path"))
                .and_then(Item::as_str)
                .map(ToOwned::to_owned)
        })
        .collect()
}

fn collect_inherited_dependency_names(
    table: &dyn TableLike,
    names: &mut Vec<String>,
    depth: usize,
) {
    for (key, item) in table.iter() {
        let Some(child) = item.as_table_like() else {
            continue;
        };
        if is_dependency_table(key) {
            for (name, dependency) in child.iter() {
                if is_workspace_inherit(dependency) {
                    names.push(name.to_string());
                }
            }
            continue;
        }
        if depth < MAX_DEPENDENCY_TABLE_DEPTH {
            collect_inherited_dependency_names(child, names, depth.saturating_add(1));
        }
    }
}

/// Collects every `path` value declared by a dependency of this package.
///
/// Cargo makes a path dependency that lives inside the workspace directory a
/// member even when the `members` list does not name it, so historical
/// membership can only match Cargo once these edges are known.
fn path_dependencies(doc: &DocumentMut) -> Vec<String> {
    let mut paths = Vec::new();
    collect_path_dependencies(doc.as_table(), &mut paths, 0);
    paths
}

/// Recursion depth that reaches every dependency table Cargo supports.
///
/// Dependency tables appear at the manifest root, one level down under
/// `[target.<cfg>]`, and one further level for the table itself, so this is the
/// deepest nesting that can hold a `path` key.
const MAX_DEPENDENCY_TABLE_DEPTH: usize = 3;

fn collect_path_dependencies(table: &dyn TableLike, paths: &mut Vec<String>, depth: usize) {
    for (key, item) in table.iter() {
        let Some(child) = item.as_table_like() else {
            continue;
        };
        if is_dependency_table(key) {
            for (_, dependency) in child.iter() {
                let Some(dependency) = dependency.as_table_like() else {
                    continue;
                };
                if let Some(path) = dependency.get("path").and_then(Item::as_str) {
                    paths.push(path.to_string());
                }
            }
            continue;
        }
        if depth < MAX_DEPENDENCY_TABLE_DEPTH {
            collect_path_dependencies(child, paths, depth.saturating_add(1));
        }
    }
}

fn is_dependency_table(key: &str) -> bool {
    matches!(
        key,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    )
}

pub(crate) fn parse_workspace_members(
    content: &str,
    path: &Path,
    case: PathCase,
) -> Result<WorkspaceMembers, AppError> {
    let doc = parse_document(path, content)?;
    let Some(workspace) = doc.get("workspace").and_then(Item::as_table_like) else {
        return Ok(WorkspaceMembers::default());
    };
    Ok(WorkspaceMembers {
        members: compile_patterns(&string_array(workspace.get("members")), case)?,
        exclude: compile_patterns(&string_array(workspace.get("exclude")), case)?,
    })
}

/// Whether `dir` (repo-relative, `/` separators) matches a workspace member pattern.
pub(crate) fn is_workspace_member(dir: &str, members: &WorkspaceMembers) -> bool {
    let dir = dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    // A non-virtual root's own package is a member whatever the lists say, so it
    // is decided before them. This is only ever asked about a directory that
    // holds a package manifest, so a virtual root cannot reach it.
    if dir.is_empty() {
        return true;
    }
    if members.exclude.iter().any(|pattern| pattern.matches(dir)) {
        return false;
    }
    // A manifest without a `members` list defines a workspace whose only member
    // is the root package, already handled above. Treating an absent list as
    // "every manifest in the repository" would pull unrelated packages into a
    // historical snapshot.
    members.members.iter().any(|pattern| pattern.matches(dir))
}

/// Whether `dir` (repo-relative, `/` separators) is excluded from the workspace.
///
/// Membership that Cargo derives from a path dependency still honours
/// `exclude`, so that list is queried separately from the `members` patterns.
pub(crate) fn is_workspace_excluded(dir: &str, members: &WorkspaceMembers) -> bool {
    let dir = dir.replace('\\', "/");
    let dir = dir.trim_end_matches('/');
    members.exclude.iter().any(|pattern| pattern.matches(dir))
}

fn compile_patterns(patterns: &[String], case: PathCase) -> Result<Vec<MemberPattern>, AppError> {
    patterns
        .iter()
        .map(|pattern| MemberPattern::new(pattern, case))
        .collect()
}

/// One compiled `[workspace] members` / `exclude` pattern.
struct MemberPattern {
    literal: String,
    case: PathCase,
    matcher: Override,
}

impl MemberPattern {
    fn new(pattern: &str, case: PathCase) -> Result<Self, AppError> {
        let literal = pattern.replace('\\', "/");
        let mut matcher = OverrideBuilder::new("");
        if case == PathCase::Insensitive {
            matcher
                .case_insensitive(true)
                .map_err(|error| InvalidMemberPatternError::caused_by(&literal, error))?;
        }
        matcher
            .add(&literal)
            .map_err(|error| InvalidMemberPatternError::caused_by(&literal, error))?;
        let matcher = matcher
            .build()
            .map_err(|error| InvalidMemberPatternError::caused_by(&literal, error))?;
        Ok(Self {
            literal,
            case,
            matcher,
        })
    }

    fn matches(&self, dir: &str) -> bool {
        if self.case.same_path(&self.literal, dir) {
            return true;
        }
        // `foo/**` in Cargo member lists includes the `foo` directory itself.
        if let Some(prefix) = self.literal.strip_suffix("/**")
            && self.case.same_path(prefix, dir)
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
        Self::new(&self.literal, self.case).unwrap_or_else(|_| Self {
            literal: self.literal.clone(),
            case: self.case,
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

fn packaging_from_package(
    package: &dyn TableLike,
    workspace: &WorkspaceInherit<'_>,
) -> Result<PackagingRules, AppError> {
    let include = inherited_string_array(package, workspace, "include");
    let exclude = inherited_string_array(package, workspace, "exclude");
    PackagingRules::new(include.as_deref(), exclude.as_deref())
}

/// Reads a `[package]` string array, following `.workspace = true` to the root.
fn inherited_string_array(
    package: &dyn TableLike,
    workspace: &WorkspaceInherit<'_>,
    key: &str,
) -> Option<Vec<String>> {
    let item = package.get(key)?;
    if is_workspace_inherit(item) {
        return opt_string_array(workspace.package_key(key));
    }
    opt_string_array(Some(item))
}

fn publish_allowed(package: &dyn TableLike, workspace: &WorkspaceInherit<'_>) -> bool {
    let Some(item) = package.get("publish") else {
        return true;
    };
    let item = if is_workspace_inherit(item) {
        // An inherited key whose root value is absent is a manifest Cargo
        // rejects, so the publishable default keeps it under the release gate.
        let Some(item) = workspace.package_key("publish") else {
            return true;
        };
        item
    } else {
        item
    };
    match item {
        Item::Value(Value::Boolean(b)) => *b.value(),
        Item::Value(Value::Array(array)) => !array.is_empty(),
        // Cargo accepts only a boolean or a registry array here, so any other
        // shape is a manifest Cargo itself rejects. Treating it as publishable
        // keeps the package under the release gate; the opposite default would
        // silently exempt a package from classification.
        _ => true,
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
        cased_members(patterns, PathCase::Sensitive)
    }

    fn cased_members(patterns: &[&str], case: PathCase) -> WorkspaceMembers {
        WorkspaceMembers {
            members: compile_patterns(
                &patterns
                    .iter()
                    .map(|pattern| (*pattern).to_string())
                    .collect::<Vec<_>>(),
                case,
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
            &WorkspaceInherit::default(),
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
            PathCase::Sensitive,
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
            PathCase::Sensitive,
        )
        .unwrap();
        assert!(is_workspace_member("packages/foo", &declared));
        assert!(!is_workspace_member("packages/skip", &declared));
        assert!(!is_workspace_member("examples/foo", &declared));
        // A non-virtual root's own package is a member even though no pattern
        // names it.
        assert!(is_workspace_member("", &declared));
        // Without a `members` list the only member is the root package, matching
        // Cargo rather than treating every manifest in the tree as a member.
        let empty = parse_workspace_members(
            "[workspace]\n",
            Path::new("Cargo.toml"),
            PathCase::Sensitive,
        )
        .unwrap();
        assert!(is_workspace_member("", &empty));
        assert!(!is_workspace_member("anything", &empty));
    }

    #[test]
    fn workspace_exclusion_is_queried_independently_of_membership() {
        let declared = parse_workspace_members(
            "[workspace]\nmembers = [\"packages/*\"]\nexclude = [\"packages/skip\"]\n",
            Path::new("Cargo.toml"),
            PathCase::Sensitive,
        )
        .unwrap();
        assert!(is_workspace_excluded("packages/skip", &declared));
        // Not named by either list: outside the workspace, but not excluded.
        assert!(!is_workspace_excluded("examples/foo", &declared));
    }

    #[test]
    fn path_dependencies_are_collected_from_every_dependency_table() {
        let parsed = parse_package_manifest(
            r#"
[package]
name = "a"
version = "0.1.0"

[dependencies]
b = { path = "../b" }
registry = "1"

[build-dependencies]
c = { path = "../c" }

[dev-dependencies]
d = { path = "../d" }

[target.'cfg(windows)'.dependencies]
e = { path = "../e" }
"#,
            "packages/a/Cargo.toml",
            &WorkspaceInherit::default(),
        )
        .unwrap()
        .unwrap();
        let mut paths = parsed.path_dependencies;
        paths.sort();
        assert_eq!(paths, vec!["../b", "../c", "../d", "../e"]);
    }

    #[test]
    fn case_insensitive_matching_follows_the_probed_filesystem() {
        let strict = cased_members(&["Packages/*"], PathCase::Sensitive);
        assert!(!is_workspace_member("packages/foo", &strict));
        assert!(is_workspace_member("Packages/foo", &strict));

        let relaxed = cased_members(&["Packages/*"], PathCase::Insensitive);
        assert!(is_workspace_member("packages/foo", &relaxed));
        assert!(is_workspace_member("Packages/foo", &relaxed));

        // The literal and `foo/**` prefix fast paths follow the same rules as
        // the compiled matcher.
        let prefix = cased_members(&["Packages/**"], PathCase::Insensitive);
        assert!(is_workspace_member("packages", &prefix));
    }

    #[cfg_attr(miri, ignore)] // Reads a real directory, which Miri cannot emulate.
    #[test]
    fn path_case_probe_agrees_with_the_filesystem() {
        let dir = tempfile::TempDir::new().unwrap();
        fs::write(dir.path().join("Probe.txt"), "x").unwrap();
        let probed = PathCase::probe(dir.path());
        let observed = if dir.path().join("PROBE.TXT").exists() {
            PathCase::Insensitive
        } else {
            PathCase::Sensitive
        };
        assert_eq!(probed, observed);
    }

    #[cfg_attr(miri, ignore)] // Reads the filesystem, which Miri cannot emulate.
    #[test]
    fn unreadable_directory_probes_as_case_sensitive() {
        // The stricter answer never widens member matching, so an unreadable
        // directory must not relax it.
        assert_eq!(
            PathCase::probe(Path::new("cargo-release-plan-no-such-directory")),
            PathCase::Sensitive
        );
    }

    #[test]
    fn flip_case_inverts_cased_characters_only() {
        assert_eq!(flip_case("Cargo.toml"), "cARGO.TOML");
        assert_eq!(flip_case("123"), "123");
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
            &WorkspaceInherit::default(),
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
            &WorkspaceInherit::default(),
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
            &WorkspaceInherit::default(),
        )
        .unwrap()
        .unwrap();
        assert!(parsed.packaging.is_released("src/lib.rs"));
        assert!(!parsed.packaging.is_released("tests/x.rs"));
    }

    #[test]
    fn inherited_workspace_version_is_parsed() {
        let root = root_doc("[workspace.package]\nversion = \"0.4.0\"\n");
        let parsed = parse_package_manifest(
            r#"
[package]
name = "foo"
version.workspace = true
"#,
            "packages/foo/Cargo.toml",
            &WorkspaceInherit::from_root(&root),
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
                &WorkspaceInherit::default(),
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn inherited_packaging_and_publish_are_resolved_from_the_root() {
        let root = root_doc(
            "[workspace.package]\ninclude = [\"src/\"]\nexclude = [\"tests/\"]\npublish = false\n",
        );
        let parsed = parse_package_manifest(
            r#"
[package]
name = "foo"
version = "0.1.0"
include.workspace = true
publish.workspace = true
"#,
            "packages/foo/Cargo.toml",
            &WorkspaceInherit::from_root(&root),
        )
        .unwrap()
        .unwrap();
        assert!(!parsed.publish);
        assert!(parsed.packaging.is_released("src/lib.rs"));
        assert!(!parsed.packaging.is_released("README.md"));
    }

    #[test]
    fn inherited_path_dependencies_resolve_against_the_workspace_root() {
        let root = root_doc(
            "[workspace.dependencies]\nb = { path = \"packages/b\", version = \"0.1.0\" }\n",
        );
        let parsed = parse_package_manifest(
            r#"
[package]
name = "a"
version = "0.1.0"

[dependencies]
b.workspace = true

[dev-dependencies]
c = { path = "../c" }
"#,
            "packages/a/Cargo.toml",
            &WorkspaceInherit::from_root(&root),
        )
        .unwrap()
        .unwrap();
        assert_eq!(parsed.inherited_path_dependencies, vec!["packages/b"]);
        assert_eq!(parsed.path_dependencies, vec!["../c"]);
    }

    fn root_doc(content: &str) -> DocumentMut {
        content.parse().unwrap()
    }
}
