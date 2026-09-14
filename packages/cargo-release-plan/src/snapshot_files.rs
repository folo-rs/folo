// Cached tree entries and raw metadata for historical package assessments.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::str;

use ohno::AppError;

use crate::NonUtf8BlobError;
use crate::git::{GitObjects, GitRepo, TreeEntry};
use crate::manifest::{PathCase, cargo_config_paths};

/// Acquires a commit's tree and metadata blobs without interpreting unrelated manifests.
///
/// Membership and installation resolution request text only when they need a manifest or
/// configuration. Batching raw objects removes per-file Git launches without making excluded
/// packages' UTF-8, TOML, or dependency declarations prerequisites for classification.
#[derive(Clone, Debug, Default)]
pub(crate) struct SnapshotFiles {
    pub(crate) entries: Vec<TreeEntry>,
    metadata: BTreeMap<String, String>,
    objects: GitObjects,
}

impl SnapshotFiles {
    pub(crate) fn load(git: &GitRepo, commit: &str, case: PathCase) -> Result<Self, AppError> {
        let entries = git.ls_tree(commit, &[""])?;
        let configurations: HashSet<String> = cargo_config_paths(git.prefix())
            .into_iter()
            .flatten()
            .collect();
        let metadata: BTreeMap<String, String> = entries
            .iter()
            .filter(|entry| is_manifest(&entry.path, case) || configurations.contains(&entry.path))
            .map(|entry| (entry.path.clone(), entry.id.clone()))
            .collect();
        let ids: BTreeSet<&str> = metadata.values().map(String::as_str).collect();
        let objects = GitObjects::read(git.root(), &ids.into_iter().collect::<Vec<_>>())?;
        Ok(Self {
            entries,
            metadata,
            objects,
        })
    }

    pub(crate) fn text(&self, commit: &str, path: &str) -> Result<Option<&str>, AppError> {
        let Some(id) = self.metadata.get(path) else {
            return Ok(None);
        };
        let bytes = self.objects.blob(id)?;
        str::from_utf8(bytes)
            .map(Some)
            .map_err(|error| NonUtf8BlobError::caused_by(commit, path, error).into())
    }
}

fn is_manifest(path: &str, case: PathCase) -> bool {
    path.rsplit('/')
        .next()
        .is_some_and(|name| case.same_path(name, "Cargo.toml"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn prefetch_keeps_manifest_spellings_that_historical_path_lookup_can_resolve() {
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            assert!(is_manifest("vendor/dependency/Cargo.toml", case));
            assert!(!is_manifest("vendor/dependency/Cargo.lock", case));
            assert!(!is_manifest("vendor/dependency/Cargo.toml/README.md", case));
        }
        assert!(!is_manifest(
            "vendor/dependency/cargo.toml",
            PathCase::Sensitive
        ));
        assert!(is_manifest(
            "vendor/dependency/cargo.toml",
            PathCase::Insensitive
        ));
    }
}
