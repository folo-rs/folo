// Captured-path comparison uses each directory's observed alias rules, not the host OS.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use crate::git::os_path;
use crate::manifest::PathCase;

/// Interprets captured names using an injected, read-only directory case probe.
///
/// The real probe observes filesystem identity. Tests supply mixed directory rules without
/// inventing candidate inputs or depending on the test host's case behavior.
pub(crate) struct PathIdentity<'a> {
    root: &'a Path,
    probe: &'a dyn Fn(&Path) -> PathCase,
}

impl<'a> PathIdentity<'a> {
    pub(crate) fn new(root: &'a Path, probe: &'a dyn Fn(&Path) -> PathCase) -> Self {
        Self { root, probe }
    }

    pub(crate) fn supports_artifact(&self, path: &Path) -> bool {
        if path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return false;
        }
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        // Unrelated basenames need no filesystem probe.
        let Some(expected) = ["Cargo.toml", "Cargo.lock"]
            .into_iter()
            .find(|expected| PathCase::Insensitive.same_path(name, expected))
        else {
            return false;
        };
        if name == expected {
            return true;
        }
        let path = self.root.join(path);
        let parent = path
            .parent()
            .expect("an artifact filename has a parent under its root");
        (self.probe)(parent).same_path(name, expected)
    }

    pub(crate) fn contains(&self, paths: &BTreeSet<PathBuf>, requested: &Path) -> bool {
        paths.contains(requested) || paths.iter().any(|path| self.same(path, requested))
    }

    pub(crate) fn same(&self, left: &Path, right: &Path) -> bool {
        if left == right {
            return true;
        }
        if !PathCase::Insensitive.same_path(&os_path(left), &os_path(right)) {
            return false;
        }
        let mut parent = self.root.to_path_buf();
        for (left, right) in left.components().zip(right.components()) {
            if left != right {
                let (Some(left), Some(right)) =
                    (left.as_os_str().to_str(), right.as_os_str().to_str())
                else {
                    return false;
                };
                if !(self.probe)(&parent).same_path(left, right) {
                    return false;
                }
            }
            parent.push(left);
        }
        true
    }

    pub(crate) fn same_set(
        &self,
        expected: &BTreeSet<PathBuf>,
        current: &BTreeSet<PathBuf>,
    ) -> bool {
        // Copying may collapse aliases, but it must neither omit nor add a distinct input.
        [(expected, current), (current, expected)]
            .into_iter()
            .all(|(paths, candidates)| paths.iter().all(|path| self.contains(candidates, path)))
    }

    pub(crate) fn replacement<'b>(
        &self,
        path: &Path,
        replacements: &'b BTreeMap<PathBuf, Vec<u8>>,
    ) -> Option<&'b [u8]> {
        if let Some(contents) = replacements.get(path) {
            return Some(contents);
        }
        for (candidate, contents) in replacements {
            if self.same(path, candidate) {
                return Some(contents);
            }
        }
        None
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn artifact_names_follow_the_containing_directory() {
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let probe = |parent: &Path| {
                assert_eq!(parent, Path::new("root/member"));
                case
            };
            let identity = PathIdentity::new(Path::new("root"), &probe);
            for path in ["member/Cargo.toml", "member/Cargo.lock"] {
                assert!(identity.supports_artifact(Path::new(path)));
            }
            for path in ["member/cargo.toml", "member/CARGO.LOCK"] {
                assert_eq!(
                    identity.supports_artifact(Path::new(path)),
                    case == PathCase::Insensitive
                );
            }
            for path in [
                "",
                "../Cargo.toml",
                "/Cargo.toml",
                "member/lib.rs",
                "member/cargo.txt",
            ] {
                assert!(!identity.supports_artifact(Path::new(path)));
            }
        }
    }

    #[test]
    fn each_differing_component_uses_its_own_parent() {
        let root = Path::new("root");
        for root_case in [PathCase::Sensitive, PathCase::Insensitive] {
            for child_case in [PathCase::Sensitive, PathCase::Insensitive] {
                let probe = |parent: &Path| match parent {
                    path if path == root => root_case,
                    path if path == root.join("Dir") => child_case,
                    _ => panic!("unexpected parent"),
                };
                let identity = PathIdentity::new(root, &probe);
                for (left, right, expected) in [
                    ("Dir/Cargo.toml", "Dir/Cargo.toml", true),
                    (
                        "Dir/Cargo.toml",
                        "dir/Cargo.toml",
                        root_case == PathCase::Insensitive,
                    ),
                    (
                        "Dir/Cargo.toml",
                        "Dir/cargo.toml",
                        child_case == PathCase::Insensitive,
                    ),
                    (
                        "Dir/Cargo.toml",
                        "dir/cargo.toml",
                        root_case == PathCase::Insensitive && child_case == PathCase::Insensitive,
                    ),
                    ("Dir/Cargo.toml", "Other/Cargo.toml", false),
                    ("Dir/Cargo.toml", "Dir/Cargo.toml/extra", false),
                    ("Dir/Cargo.toml", "Dir", false),
                ] {
                    assert_eq!(identity.same(Path::new(left), Path::new(right)), expected);
                }
            }
        }
    }

    #[test]
    fn membership_allows_alias_collapse_but_never_distinct_additions_or_removals() {
        let original = PathBuf::from("Cargo.toml");
        let alias = PathBuf::from("cargo.toml");
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let probe = |_: &Path| case;
            let identity = PathIdentity::new(Path::new("root"), &probe);
            let paths = BTreeSet::from([original.clone()]);
            assert!(identity.same_set(&paths, &paths));
            assert!(identity.contains(&paths, &original));
            assert_eq!(
                identity.contains(&paths, &alias),
                case == PathCase::Insensitive
            );
            for candidates in [
                BTreeSet::from([alias.clone()]),
                BTreeSet::from([original.clone(), alias.clone()]),
            ] {
                assert_eq!(
                    identity.same_set(&paths, &candidates),
                    case == PathCase::Insensitive
                );
                assert_eq!(
                    identity.same_set(&candidates, &paths),
                    case == PathCase::Insensitive
                );
            }
            for candidates in [
                BTreeSet::new(),
                BTreeSet::from([original.clone(), PathBuf::from("extra.rs")]),
            ] {
                assert!(!identity.same_set(&paths, &candidates));
                assert!(!identity.same_set(&candidates, &paths));
            }
        }
    }

    #[test]
    fn replacements_use_exact_names_before_aliases_and_preserve_distinct_files() {
        let replacements = BTreeMap::from([
            (PathBuf::from("Cargo.toml"), b"original".to_vec()),
            (PathBuf::from("cargo.toml"), b"distinct".to_vec()),
        ]);
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let probe = |_: &Path| case;
            let identity = PathIdentity::new(Path::new("root"), &probe);
            assert_eq!(
                identity.replacement(Path::new("cargo.toml"), &replacements),
                Some(&b"distinct"[..])
            );
            assert_eq!(
                identity.replacement(Path::new("CARGO.TOML"), &replacements),
                (case == PathCase::Insensitive).then_some(&b"original"[..])
            );
            assert_eq!(
                identity.replacement(Path::new("Cargo.lock"), &replacements),
                None
            );
        }
    }
}
