//! Fresh classification-pass listings, independent of committed observation storage.

use std::collections::BTreeSet;
use std::env;

use ohno::AppError;

use crate::git::{GitRepo, WorkTreeModes};
use crate::manifest::PathCase;

/// One pass's tracked paths, effective index/worktree modes and scoped untracked paths.
///
/// The tracked listing belongs to the same metadata acquisition. Package selection overlaps:
/// resources and nested directories may be consumed by more than one package. This value must
/// not survive edits, resolution or another classification boundary.
#[derive(Debug)]
pub struct LiveObservations<'a> {
    tracked: &'a [String],
    modes: WorkTreeModes,
    untracked: Vec<String>,
    mode_scopes: Vec<String>,
    untracked_scopes: Vec<String>,
    case: PathCase,
    standard_pathspecs: bool,
}

impl<'a> LiveObservations<'a> {
    #[cfg_attr(test, mutants::skip)] // Native acquisition; acquire_with and selection are pure.
    pub fn acquire(
        git: &GitRepo,
        tracked: &'a [String],
        directories: &[&str],
        resources: &[&str],
        case: PathCase,
    ) -> Result<Self, AppError> {
        // These variables can reinterpret even explicit magic. Preserve narrow Git queries
        // rather than claiming that local prefix matching models an overridden pathspec parser.
        let standard_pathspecs = [
            "GIT_LITERAL_PATHSPECS",
            "GIT_GLOB_PATHSPECS",
            "GIT_NOGLOB_PATHSPECS",
            "GIT_ICASE_PATHSPECS",
        ]
        .iter()
        .all(|name| env::var_os(name).is_none());
        Self::acquire_with(
            tracked,
            directories,
            resources,
            case,
            standard_pathspecs,
            |paths| {
                let scopes: Vec<_> = directories.iter().chain(resources).copied().collect();
                let relevant =
                    Self::select_paths(tracked, &scopes, case, standard_pathspecs, || {
                        git.tracked_paths(&scopes, case)
                    })?;
                let relevant: Vec<_> = relevant.iter().map(String::as_str).collect();
                // Even a raw diff can execute a clean driver for a racily clean index entry.
                // Preserve per-package mode-query ordering if any relevant driver is selected.
                // Ref: docs/implementation.md, "Fresh classification listings".
                Self::filter_safe_modes(
                    || git.may_have_filter_drivers(&relevant),
                    || git.work_tree_modes(paths, case),
                )
            },
            |paths| git.untracked_paths(paths, case),
        )
    }

    fn filter_safe_modes(
        drivers: impl FnOnce() -> Result<bool, AppError>,
        modes: impl FnOnce() -> Result<WorkTreeModes, AppError>,
    ) -> Result<Option<WorkTreeModes>, AppError> {
        if drivers()? {
            Ok(None)
        } else {
            modes().map(Some)
        }
    }

    fn acquire_with(
        tracked: &'a [String],
        directories: &[&str],
        resources: &[&str],
        case: PathCase,
        standard_pathspecs: bool,
        modes: impl FnOnce(&[&str]) -> Result<Option<WorkTreeModes>, AppError>,
        untracked: impl FnOnce(&[&str]) -> Result<Vec<String>, AppError>,
    ) -> Result<Self, AppError> {
        let directories: BTreeSet<_> = directories
            .iter()
            .copied()
            .filter(|path| shareable(path, case, standard_pathspecs))
            .collect();
        let paths: Vec<_> = directories
            .iter()
            .copied()
            .chain(
                resources
                    .iter()
                    .copied()
                    .filter(|path| shareable(path, case, standard_pathspecs)),
            )
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let directories: Vec<_> = directories.into_iter().collect();
        let modes = if paths.is_empty() {
            None
        } else {
            modes(&paths)?
        };
        let mode_scopes = if modes.is_some() {
            paths.into_iter().map(str::to_owned).collect()
        } else {
            Vec::new()
        };
        Ok(Self {
            tracked,
            modes: modes.unwrap_or_default(),
            untracked: if directories.is_empty() {
                Vec::new()
            } else {
                untracked(&directories)?
            },
            mode_scopes,
            untracked_scopes: directories.into_iter().map(str::to_owned).collect(),
            case,
            standard_pathspecs,
        })
    }

    /// Selects the same literal Git scope without substituting filesystem Unicode case rules.
    pub fn tracked_paths(
        &self,
        paths: &[&str],
        narrow: impl FnOnce() -> Result<Vec<String>, AppError>,
    ) -> Result<Vec<String>, AppError> {
        self.select(self.tracked, paths, narrow)
    }

    /// Selects untracked candidates; packaging, presence and nested boundaries remain caller-owned.
    pub fn untracked_paths(
        &self,
        directory: &str,
        narrow: impl FnOnce() -> Result<Vec<String>, AppError>,
    ) -> Result<Vec<String>, AppError> {
        if !self.covered(&[directory], &self.untracked_scopes) {
            return narrow();
        }
        self.select(&self.untracked, &[directory], narrow)
    }

    /// Effective modes share the index baseline and worktree overlay acquired for this pass.
    pub fn modes(
        &self,
        paths: &[&str],
        narrow: impl FnOnce() -> Result<WorkTreeModes, AppError>,
    ) -> Result<WorkTreeModes, AppError> {
        if !self.can_share(paths) || !self.covered(paths, &self.mode_scopes) {
            return narrow();
        }
        Ok(WorkTreeModes {
            executable: self
                .modes
                .executable
                .iter()
                .filter(|path| self.matches(path, paths))
                .cloned()
                .collect(),
            symlinks: self
                .modes
                .symlinks
                .iter()
                .filter(|path| self.matches(path, paths))
                .cloned()
                .collect(),
        })
    }

    fn select(
        &self,
        listed: &[String],
        paths: &[&str],
        narrow: impl FnOnce() -> Result<Vec<String>, AppError>,
    ) -> Result<Vec<String>, AppError> {
        Self::select_paths(listed, paths, self.case, self.standard_pathspecs, narrow)
    }

    fn select_paths(
        listed: &[String],
        scopes: &[&str],
        case: PathCase,
        standard_pathspecs: bool,
        narrow: impl FnOnce() -> Result<Vec<String>, AppError>,
    ) -> Result<Vec<String>, AppError> {
        if !scopes
            .iter()
            .all(|path| shareable(path, case, standard_pathspecs))
        {
            return narrow();
        }
        Ok(listed
            .iter()
            .filter(|path| {
                scopes
                    .iter()
                    .any(|scope| literal_matches(path, scope, case))
            })
            .cloned()
            .collect())
    }

    fn can_share(&self, paths: &[&str]) -> bool {
        paths
            .iter()
            .all(|path| shareable(path, self.case, self.standard_pathspecs))
    }

    fn matches(&self, path: &str, scopes: &[&str]) -> bool {
        scopes
            .iter()
            .any(|scope| literal_matches(path, scope, self.case))
    }

    fn covered(&self, paths: &[&str], scopes: &[String]) -> bool {
        paths.iter().all(|path| {
            scopes
                .iter()
                .any(|scope| literal_matches(path, scope, self.case))
        })
    }
}

fn shareable(path: &str, case: PathCase, standard_pathspecs: bool) -> bool {
    standard_pathspecs
        && (case == PathCase::Sensitive || path.is_ascii())
        && (path.is_empty() || path.split('/').all(|part| !matches!(part, "" | "." | "..")))
}

fn literal_matches(path: &str, scope: &str, case: PathCase) -> bool {
    if scope.is_empty() {
        return true;
    }
    let path = path.as_bytes();
    let scope = scope.as_bytes();
    let Some(prefix) = path.get(..scope.len()) else {
        return false;
    };
    let same = match case {
        PathCase::Sensitive => prefix == scope,
        // Git's literal icase matching folds ASCII bytes, not Unicode characters.
        // Non-ASCII scope spellings retain the native query (shareable).
        PathCase::Insensitive => prefix.eq_ignore_ascii_case(scope),
    };
    same && (path.len() == scope.len() || path.get(scope.len()) == Some(&b'/'))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::Cell;
    use std::io;

    use super::*;

    #[test]
    fn overlapping_packages_share_acquisitions_and_preserve_literal_boundaries() {
        let tracked = [
            "a/src/lib.rs",
            "a/nested/lib.rs",
            "ab/lib.rs",
            "b/lib.rs",
            "shared/LICENSE",
        ]
        .map(str::to_owned);
        let modes_count = Cell::new(0);
        let untracked_count = Cell::new(0);
        let observed = LiveObservations::acquire_with(
            &tracked,
            &["b", "a", "a/nested"],
            &["shared/LICENSE", "shared/LICENSE"],
            PathCase::Sensitive,
            true,
            |paths| {
                modes_count.set(modes_count.get() + 1);
                assert_eq!(paths, ["a", "a/nested", "b", "shared/LICENSE"]);
                let mut modes = WorkTreeModes::default();
                modes.set("shared/LICENSE", "100755");
                modes.set("a/src/lib.rs", "100755");
                modes.set("b/lib.rs", "120000");
                Ok(Some(modes))
            },
            |paths| {
                untracked_count.set(untracked_count.get() + 1);
                assert_eq!(paths, ["a", "a/nested", "b"]);
                Ok(["a/new", "a/nested/new", "b/new"]
                    .map(str::to_owned)
                    .to_vec())
            },
        )
        .unwrap();
        for directory in ["a", "b"] {
            let paths = [directory, "shared/LICENSE"];
            assert!(
                observed
                    .tracked_paths(&paths, || panic!("shared"))
                    .unwrap()
                    .contains(&"shared/LICENSE".to_owned())
            );
            assert!(
                observed
                    .modes(&paths, || panic!("shared"))
                    .unwrap()
                    .is_executable("shared/LICENSE")
            );
            let modes = observed.modes(&paths, || panic!("shared")).unwrap();
            assert_eq!(modes.is_executable("a/src/lib.rs"), directory == "a");
            assert_eq!(modes.is_symlink("b/lib.rs"), directory == "b");
        }
        assert_eq!(
            observed.tracked_paths(&["a"], || panic!("shared")).unwrap(),
            ["a/src/lib.rs", "a/nested/lib.rs"]
        );
        assert_eq!(
            observed
                .untracked_paths("a/nested", || panic!("shared"))
                .unwrap(),
            ["a/nested/new"]
        );
        assert_eq!((modes_count.get(), untracked_count.get()), (1, 1));
    }

    #[test]
    fn uncertain_scopes_retain_narrow_acquisition_and_errors() {
        for (case, standard, scope) in [
            (PathCase::Insensitive, true, "ä"),
            (PathCase::Sensitive, false, "ascii"),
            (PathCase::Sensitive, true, "a/../b"),
        ] {
            let observed = LiveObservations::acquire_with(
                &[],
                &[scope],
                &[],
                case,
                standard,
                |_| panic!("no shared scope"),
                |_| panic!("no shared scope"),
            )
            .unwrap();
            assert_eq!(
                observed
                    .tracked_paths(&[scope], || Ok(vec!["native".into()]))
                    .unwrap(),
                ["native"]
            );
            observed
                .untracked_paths(scope, || Err(io::Error::other("untracked").into()))
                .unwrap_err();
            observed
                .modes(&[scope], || Err(io::Error::other("modes").into()))
                .unwrap_err();
        }
        LiveObservations::acquire_with(
            &[],
            &["a"],
            &[],
            PathCase::Sensitive,
            true,
            |_| Err(io::Error::other("index").into()),
            |_| panic!("index failed"),
        )
        .unwrap_err();
        LiveObservations::acquire_with(
            &[],
            &["a"],
            &[],
            PathCase::Sensitive,
            true,
            |_| Ok(Some(WorkTreeModes::default())),
            |_| Err(io::Error::other("others").into()),
        )
        .unwrap_err();
    }

    #[test]
    fn safe_case_scopes_share_but_modes_and_untracked_require_coverage() {
        for (case, scope) in [(PathCase::Sensitive, "ä"), (PathCase::Insensitive, "a")] {
            let tracked = [format!("{scope}/file")];
            let observed = LiveObservations::acquire_with(
                &tracked,
                &[scope],
                &[],
                case,
                true,
                |paths| {
                    assert_eq!(paths, [scope]);
                    let mut modes = WorkTreeModes::default();
                    modes.set(&tracked[0], "100755");
                    Ok(Some(modes))
                },
                |paths| {
                    assert_eq!(paths, [scope]);
                    Ok(vec![format!("{scope}/new")])
                },
            )
            .unwrap();
            assert_eq!(
                observed
                    .tracked_paths(&[scope], || panic!("shared"))
                    .unwrap(),
                tracked
            );
            assert!(
                observed
                    .modes(&[scope], || panic!("shared"))
                    .unwrap()
                    .is_executable(&tracked[0])
            );
            for outside in ["", "outside"] {
                observed
                    .modes(&[outside], || Err(io::Error::other("outside modes").into()))
                    .unwrap_err();
                observed
                    .untracked_paths(outside, || Err(io::Error::other("outside others").into()))
                    .unwrap_err();
            }
            // Coverage alone cannot admit a scope whose Git case behavior is not shared.
            if case == PathCase::Insensitive {
                observed
                    .modes(&["a/ä"], || Err(io::Error::other("native case").into()))
                    .unwrap_err();
            }
        }
    }

    #[test]
    fn declined_mode_sharing_keeps_each_packages_narrow_query() {
        let observed = LiveObservations::acquire_with(
            &[],
            &["a", "b"],
            &[],
            PathCase::Sensitive,
            true,
            |_| Ok(None),
            |_| Ok(vec!["a/new".into()]),
        )
        .unwrap();
        let calls = Cell::new(0);
        for directory in ["a", "b"] {
            observed
                .modes(&[directory], || {
                    calls.set(calls.get() + 1);
                    Ok(WorkTreeModes::default())
                })
                .unwrap();
        }
        assert_eq!(calls.get(), 2);
        assert_eq!(
            observed.untracked_paths("a", || panic!("shared")).unwrap(),
            ["a/new"]
        );
    }

    #[test]
    fn filter_admission_precedes_modes_and_propagates_query_errors() {
        assert!(
            LiveObservations::filter_safe_modes(|| Ok(true), || panic!("driver selected"))
                .unwrap()
                .is_none()
        );
        assert_eq!(
            LiveObservations::filter_safe_modes(|| Ok(false), || Ok(WorkTreeModes::default()))
                .unwrap(),
            Some(WorkTreeModes::default())
        );
        LiveObservations::filter_safe_modes(
            || Err(io::Error::other("attributes").into()),
            || panic!("attribute query failed"),
        )
        .unwrap_err();
        LiveObservations::filter_safe_modes(|| Ok(false), || Err(io::Error::other("index").into()))
            .unwrap_err();
    }

    #[test]
    fn matching_is_component_bounded_literal_and_not_unicode_folding() {
        for (path, scope, sensitive, insensitive) in [
            ("ab/file", "a", false, false),
            ("a/file", "ab", false, false),
            ("a/file", "a", true, true),
            ("a", "a", true, true),
            ("A/file", "a", false, true),
            ("a/[b]/file", "a/[b]", true, true),
            ("a/b/file", "a/[b]", false, false),
            ("ä/file", "a", false, false),
            ("a/ä", "a", true, true),
            ("a/file", "", true, true),
        ] {
            assert_eq!(literal_matches(path, scope, PathCase::Sensitive), sensitive);
            assert_eq!(
                literal_matches(path, scope, PathCase::Insensitive),
                insensitive
            );
        }
    }
}
