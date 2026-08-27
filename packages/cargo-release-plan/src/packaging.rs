// Released-content matching.
//
// A package's released files are git-tracked paths under its directory, filtered
// by the manifest `include` / `exclude` using gitignore-style matching (the
// `ignore` crate). `Cargo.lock` is never released content: the published lockfile
// is derived per-crate at pack time and is not a function of the package source.

use ignore::gitignore::GitignoreBuilder;

/// Include / exclude rules from a package manifest.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PackagingRules {
    include: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

impl PackagingRules {
    pub(crate) fn new(include: Option<Vec<String>>, exclude: Option<Vec<String>>) -> Self {
        Self { include, exclude }
    }

    /// Whether `package_relative_path` would be put in the `.crate`.
    ///
    /// `include` is an allow-list (and `exclude` is then ignored, matching Cargo).
    /// `Cargo.toml` is always released. `Cargo.lock` is never released.
    pub(crate) fn is_released(&self, package_relative_path: &str) -> bool {
        let path = normalize_rel(package_relative_path);
        if is_cargo_lock(&path) {
            return false;
        }
        if path == "Cargo.toml" {
            return true;
        }
        if let Some(include) = &self.include {
            return matches_gitignore(include, &path);
        }
        if let Some(exclude) = &self.exclude {
            return !matches_gitignore(exclude, &path);
        }
        true
    }
}

fn is_cargo_lock(path: &str) -> bool {
    path == "Cargo.lock" || path.ends_with("/Cargo.lock")
}

fn normalize_rel(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn matches_gitignore(patterns: &[String], path: &str) -> bool {
    let mut builder = GitignoreBuilder::new("");
    for pattern in patterns {
        if builder.add_line(None, pattern).is_err() {
            continue;
        }
    }
    let Ok(gitignore) = builder.build() else {
        return false;
    };
    gitignore.matched(path, false).is_ignore()
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

    #[test]
    fn cargo_toml_is_always_released() {
        let rules = PackagingRules::new(Some(vec!["src/**".to_string()]), None);
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
        let rules = PackagingRules::new(
            Some(vec!["src/**".to_string(), "README.md".to_string()]),
            None,
        );
        assert!(rules.is_released("src/lib.rs"));
        assert!(rules.is_released("README.md"));
        assert!(!rules.is_released("tests/foo.rs"));
        assert!(!rules.is_released("benches/foo.rs"));
    }

    #[test]
    fn include_later_negation_drops_a_subset() {
        let rules = PackagingRules::new(
            Some(vec!["src/**".to_string(), "!src/private/**".to_string()]),
            None,
        );
        assert!(rules.is_released("src/lib.rs"));
        assert!(!rules.is_released("src/private/x.rs"));
    }

    #[test]
    fn exclude_drops_matching_paths_when_include_absent() {
        let rules = PackagingRules::new(None, Some(vec!["tests/**".to_string()]));
        assert!(rules.is_released("src/lib.rs"));
        assert!(!rules.is_released("tests/foo.rs"));
    }

    #[test]
    fn include_ignores_exclude() {
        let rules = PackagingRules::new(
            Some(vec!["src/**".to_string(), "tests/**".to_string()]),
            Some(vec!["tests/**".to_string()]),
        );
        assert!(rules.is_released("tests/foo.rs"));
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
