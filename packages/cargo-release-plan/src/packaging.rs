// Released-content matching.
//
// A package's released files are git-tracked paths under its directory, filtered
// by the manifest `include` / `exclude` using gitignore-style matching (the
// `ignore` crate). `Cargo.lock` is never released content: the published lockfile
// is derived per-crate at pack time and is not a function of the package source.

use std::borrow::Cow;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ohno::AppError;

use crate::InvalidPackagingPatternError;

/// Include / exclude rules from a package manifest.
///
/// Matchers are compiled once when the rules are parsed so classification can
/// query many paths without rebuilding gitignore state per file.
#[derive(Clone, Debug, Default)]
pub(crate) struct PackagingRules {
    include: Option<Gitignore>,
    exclude: Option<Gitignore>,
}

impl PackagingRules {
    pub(crate) fn new(
        include: Option<&[String]>,
        exclude: Option<&[String]>,
    ) -> Result<Self, AppError> {
        Ok(Self {
            include: include.map(compile_gitignore).transpose()?,
            exclude: exclude.map(compile_gitignore).transpose()?,
        })
    }

    /// Whether `package_relative_path` would be put in the `.crate`.
    ///
    /// `include` is an allow-list (and `exclude` is then ignored, matching Cargo).
    /// `Cargo.toml` is always released. `Cargo.lock` is never released.
    pub(crate) fn is_released(&self, package_relative_path: &str) -> bool {
        let path = normalize_rel(package_relative_path);
        let path = path.as_ref();
        if is_cargo_lock(path) {
            return false;
        }
        if path == "Cargo.toml" {
            return true;
        }
        if let Some(include) = &self.include {
            return include.matched(path, false).is_ignore();
        }
        if let Some(exclude) = &self.exclude {
            return !exclude.matched(path, false).is_ignore();
        }
        true
    }
}

fn is_cargo_lock(path: &str) -> bool {
    path == "Cargo.lock" || path.ends_with("/Cargo.lock")
}

/// Rewrites a path into the `/`-separated form the matchers expect.
///
/// Classification queries every tracked path of every package, and paths that
/// already use `/` are the common case, so the already-normalized input borrows
/// instead of allocating.
fn normalize_rel(path: &str) -> Cow<'_, str> {
    let trimmed = path.trim_start_matches("./");
    if trimmed.contains('\\') {
        Cow::Owned(trimmed.replace('\\', "/"))
    } else {
        Cow::Borrowed(trimmed)
    }
}

fn compile_gitignore(patterns: &[String]) -> Result<Gitignore, AppError> {
    let mut builder = GitignoreBuilder::new("");
    for pattern in patterns {
        builder
            .add_line(None, pattern)
            .map_err(|error| InvalidPackagingPatternError::caused_by(pattern, error))?;
    }
    builder
        .build()
        .map_err(|error| InvalidPackagingPatternError::caused_by("include/exclude", error).into())
}

/// Relative path of `full` inside `package_dir`, both repo-relative with `/`.
pub(crate) fn relativize<'a>(full: &'a str, package_dir: &str) -> Option<&'a str> {
    let full = full.trim_start_matches("./");
    if package_dir.is_empty() || package_dir == "." {
        return Some(full);
    }
    let prefix = package_dir.trim_end_matches('/');
    if full == prefix {
        return None;
    }
    full.strip_prefix(prefix)?.strip_prefix('/')
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::InvalidPackagingPatternError;

    fn rules(
        include: Option<&[&str]>,
        exclude: Option<&[&str]>,
    ) -> Result<PackagingRules, AppError> {
        let include = include.map(|patterns| {
            patterns
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect::<Vec<_>>()
        });
        let exclude = exclude.map(|patterns| {
            patterns
                .iter()
                .map(|pattern| (*pattern).to_string())
                .collect::<Vec<_>>()
        });
        PackagingRules::new(include.as_deref(), exclude.as_deref())
    }

    #[test]
    fn cargo_toml_is_always_released() {
        let rules = rules(Some(&["src/**"]), None).unwrap();
        assert!(rules.is_released("Cargo.toml"));
    }

    #[test]
    fn cargo_lock_is_never_released() {
        let rules = PackagingRules::default();
        assert!(!rules.is_released("Cargo.lock"));
        assert!(!rules.is_released("nested/Cargo.lock"));
    }

    #[test]
    fn include_allow_list_keeps_matching_paths() {
        let rules = rules(Some(&["src/**", "README.md"]), None).unwrap();
        assert!(rules.is_released("src/lib.rs"));
        assert!(rules.is_released("README.md"));
        assert!(!rules.is_released("tests/foo.rs"));
        assert!(!rules.is_released("benches/foo.rs"));
    }

    #[test]
    fn include_later_negation_drops_a_subset() {
        let rules = rules(Some(&["src/**", "!src/private/**"]), None).unwrap();
        assert!(rules.is_released("src/lib.rs"));
        assert!(!rules.is_released("src/private/x.rs"));
    }

    #[test]
    fn exclude_drops_matching_paths_when_include_absent() {
        let rules = rules(None, Some(&["tests/**"])).unwrap();
        assert!(rules.is_released("src/lib.rs"));
        assert!(!rules.is_released("tests/foo.rs"));
    }

    #[test]
    fn include_ignores_exclude() {
        let rules = rules(Some(&["src/**", "tests/**"]), Some(&["tests/**"])).unwrap();
        assert!(rules.is_released("tests/foo.rs"));
    }

    #[test]
    fn invalid_include_pattern_is_an_error() {
        let error = rules(Some(&["foo.{js,ts"]), None).unwrap_err();
        let source = error
            .find_source::<InvalidPackagingPatternError>()
            .expect("invalid packaging pattern");
        assert_eq!(source.pattern(), "foo.{js,ts");
    }

    #[test]
    fn no_rules_releases_everything_but_lockfile() {
        let rules = PackagingRules::default();
        assert!(rules.is_released("src/lib.rs"));
        assert!(rules.is_released("tests/foo.rs"));
        assert!(!rules.is_released("Cargo.lock"));
    }

    #[test]
    fn relativize_strips_package_dir() {
        assert_eq!(
            relativize("packages/foo/src/lib.rs", "packages/foo"),
            Some("src/lib.rs")
        );
        assert_eq!(
            relativize("packages/foo/Cargo.toml", "packages/foo"),
            Some("Cargo.toml")
        );
        assert_eq!(relativize("packages/bar/src/lib.rs", "packages/foo"), None);
        // Empty and `.` are both "workspace root as package dir"; each arm must
        // independently return the full path so `||` cannot become `&&`.
        assert_eq!(relativize("src/lib.rs", ""), Some("src/lib.rs"));
        assert_eq!(relativize("src/lib.rs", "."), Some("src/lib.rs"));
        assert_eq!(relativize("src/lib.rs", "src"), Some("lib.rs"));
    }
}
