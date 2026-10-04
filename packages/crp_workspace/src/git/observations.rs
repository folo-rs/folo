//! Immutable Git observations, separate from live ref and history-availability decisions.

use std::collections::HashMap;
use std::path::PathBuf;
use std::{env, fs, io};

use crp_diag::Verbose;
use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::ReadFileError;
use crate::cache::{Cache, CacheEntry};
use crate::command::run_capture;
use crate::git::{GitRepo, TreeEntry, strip_terminator};

/// A full recorded tree and shared indexes, independent of package selection.
#[derive(Clone, Debug, Default)]
pub struct HistoricalTree {
    entries: Vec<TreeEntry>,
    paths: Vec<String>,
    by_path: HashMap<String, usize>,
}

impl HistoricalTree {
    #[must_use]
    pub fn new(entries: Vec<TreeEntry>) -> Self {
        Self {
            paths: entries.iter().map(|entry| entry.path.clone()).collect(),
            by_path: entries
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.path.clone(), index))
                .collect(),
            entries,
        }
    }

    #[must_use]
    pub fn entry(&self, path: &str) -> Option<&TreeEntry> {
        self.by_path.get(path).map(|index| {
            self.entries
                .get(*index)
                .expect("the index is built from this immutable entries vector")
        })
    }

    #[must_use]
    pub fn entries(&self) -> &[TreeEntry] {
        &self.entries
    }

    #[must_use]
    pub fn paths(&self) -> &[String] {
        &self.paths
    }

    #[cfg_attr(test, mutants::skip)] // Native acquisition and storage; typed admission is pure.
    pub fn load(
        git: &GitRepo,
        commit: &str,
        context: &GitObjectContext,
        cache: &Cache,
        verbose: Verbose<'_>,
    ) -> Result<Self, AppError> {
        let key = context.key(commit)?;
        let acquire = || Ok(TreeObservation(git.ls_tree(commit, &[])?));
        let tree = if context.portable() {
            cache.get(&key, verbose, acquire)?
        } else {
            acquire()?
        };
        Ok(Self::new(tree.0))
    }
}

/// Fresh interpretation inputs binding invocation memory to Git's effective object view.
///
/// Replacements and grafts can refer to unavailable objects. Their availability is not
/// immutable, so those histories retain invocation reuse but bypass persistent observations.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct GitObjectContext {
    format: String,
    replacements: String,
    grafts: Vec<u8>,
    replacement_base: Option<String>,
    replacements_disabled: bool,
}

impl GitObjectContext {
    /// Reuses a bounded immutable content batch without retaining it beyond its consumer.
    #[cfg_attr(test, mutants::skip)] // Storage and Git adapters; entry admission is shared.
    pub fn blobs(
        &self,
        git: &GitRepo,
        ids: &[&str],
        cache: &Cache,
        verbose: Verbose<'_>,
    ) -> Result<Vec<Vec<u8>>, AppError> {
        let acquire = || {
            let blobs = match ids {
                [id] => vec![git.show_blob_bytes(id)?],
                _ => git.show_blob_batch(ids)?,
            };
            Ok(BlobObservation(blobs))
        };
        // A singleton can be oversized or have no queried size. Avoid another size query
        // and unbounded JSON serialization; multi-object batches obey the reader's budget.
        let reusable = self.portable() && ids.len() > 1;
        if !reusable {
            return acquire().map(|value| value.0);
        }
        let key = ids
            .iter()
            .map(|id| self.key(id))
            .collect::<Result<_, _>>()?;
        cache.get(&key, verbose, acquire).map(|value| value.0)
    }

    #[cfg_attr(test, mutants::skip)] // Acquires Git/environment/filesystem interpretation inputs.
    pub fn capture(git: &GitRepo) -> Result<Self, AppError> {
        let replacement_base = env::var("GIT_REPLACE_REF_BASE").ok();
        let replacements_disabled = env::var_os("GIT_NO_REPLACE_OBJECTS").is_some();
        let replacements = run_capture(
            "git",
            &[
                "for-each-ref",
                "--format=%(refname) %(objectname)",
                replacement_base.as_deref().unwrap_or("refs/replace/"),
            ],
            git.root(),
        )?;
        let graft_path = run_capture(
            "git",
            &["rev-parse", "--git-path", "info/grafts"],
            git.root(),
        )?;
        let graft_path = git
            .root()
            .join(PathBuf::from(strip_terminator(&graft_path)));
        let grafts = match fs::read(&graft_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(ReadFileError::caused_by(graft_path, error).into()),
        };
        Ok(Self {
            format: run_capture("git", &["rev-parse", "--show-object-format"], git.root())?,
            replacements,
            grafts,
            replacement_base,
            replacements_disabled,
        })
    }

    fn portable(&self) -> bool {
        self.replacements.is_empty() && self.grafts.is_empty()
    }

    fn key(&self, commit: &str) -> Result<ObjectKey, AppError> {
        self.validate_identity(commit)?;
        Ok(ObjectKey {
            commit: commit.to_owned(),
            context: self.clone(),
        })
    }

    fn validate_identity(&self, commit: &str) -> Result<(), AppError> {
        // Full Git object names are the cache identity, never refs or revision expressions.
        let valid_length = match self.format.trim() {
            "sha1" => commit.len() == 40,
            "sha256" => commit.len() == 64,
            _ => false,
        };
        if !valid_length || !commit.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(UnresolvedObjectIdentity::new(commit).into());
        }
        Ok(())
    }
}

/// Exact object identity plus the interpretation in which its facts were acquired.
#[derive(Debug, Serialize)]
pub struct ObjectKey {
    commit: String,
    context: GitObjectContext,
}

/// Retained parent-presence facts, separate from parent identities and availability.
#[derive(Debug, Default)]
pub struct CommitHeaders {
    parents: HashMap<String, bool>,
}

impl CommitHeaders {
    pub fn clear(&mut self) {
        self.parents.clear();
    }

    #[cfg_attr(test, mutants::skip)] // Native acquisition; parent_with proves reuse and failures.
    pub fn has_parent(
        &mut self,
        git: &GitRepo,
        commit: &str,
        context: &GitObjectContext,
        cache: &Cache,
        verbose: Verbose<'_>,
    ) -> Result<bool, AppError> {
        context.validate_identity(commit)?;
        self.parent_with(commit, || {
            let acquire = || git.commit_has_parent_header(commit).map(ParentObservation);
            let parent = if context.portable() {
                cache.get(&context.key(commit)?, verbose, acquire)?
            } else {
                acquire()?
            };
            Ok(parent.0)
        })
    }

    /// Shares a successful header observation from the caller's acquisition boundary.
    pub fn parent_with(
        &mut self,
        commit: &str,
        acquire: impl FnOnce() -> Result<bool, AppError>,
    ) -> Result<bool, AppError> {
        if let Some(parent) = self.parents.get(commit) {
            return Ok(*parent);
        }
        let parent = acquire()?;
        self.parents.insert(commit.to_owned(), parent);
        Ok(parent)
    }
}

/// Persisted raw tree records; derived indexes are reconstructed once per acquired snapshot.
#[derive(Deserialize, Serialize)]
struct TreeObservation(Vec<TreeEntry>);

impl CacheEntry for TreeObservation {
    const SUBJECT: &'static str = "git-trees";
    const REVISION: u32 = 1;
    type Key = ObjectKey;
}

/// Whether the immutable commit header contains a parent, not whether that parent is available.
#[derive(Deserialize, Serialize)]
struct ParentObservation(bool);

impl CacheEntry for ParentObservation {
    const SUBJECT: &'static str = "git-parent-headers";
    const REVISION: u32 = 1;
    type Key = ObjectKey;
}

/// Exact binary content of an ordered, byte-bounded set of immutable objects.
#[derive(Deserialize, Serialize)]
struct BlobObservation(Vec<Vec<u8>>);

impl CacheEntry for BlobObservation {
    const SUBJECT: &'static str = "git-blob-batches";
    const REVISION: u32 = 1;
    type Key = Vec<ObjectKey>;
}

#[ohno::error]
#[display("immutable cache observations require a full resolved Git object identity: {commit}")]
struct UnresolvedObjectIdentity {
    commit: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn headers_reuse_successful_false_and_true_results_until_cleared() {
        let mut headers = CommitHeaders::default();
        let mut acquisitions = 0;
        for _ in 0..2 {
            for (commit, parent) in [("root", false), ("child", true)] {
                assert_eq!(
                    headers
                        .parent_with(commit, || {
                            acquisitions += 1;
                            Ok(parent)
                        })
                        .unwrap(),
                    parent
                );
            }
        }
        assert_eq!(acquisitions, 2);
        headers.clear();
        assert!(headers.parents.is_empty());
        headers
            .parent_with("failed", || Err(io::Error::other("header").into()))
            .unwrap_err();
        assert!(!headers.parents.contains_key("failed"));
        assert!(headers.parent_with("root", || Ok(true)).unwrap());
    }

    #[test]
    fn interpretation_changes_change_identity_and_nonportable_views_bypass_disk() {
        let context = GitObjectContext {
            format: "sha1".into(),
            ..GitObjectContext::default()
        };
        let commit = "a".repeat(40);
        assert!(context.portable());
        let key = serde_json::to_string(&context.key(&commit).unwrap()).unwrap();
        assert_ne!(
            key,
            serde_json::to_string(&context.key(&"b".repeat(40)).unwrap()).unwrap()
        );
        for invalid in ["HEAD", "main", "", "bad"] {
            assert!(
                context
                    .key(invalid)
                    .unwrap_err()
                    .find_source::<UnresolvedObjectIdentity>()
                    .is_some()
            );
        }
        context.key(&"z".repeat(40)).unwrap_err();
        for changed in [
            GitObjectContext {
                replacements: "replacement".into(),
                ..context.clone()
            },
            GitObjectContext {
                grafts: b"graft".to_vec(),
                ..context.clone()
            },
            GitObjectContext {
                replacement_base: Some("refs/custom/".into()),
                ..context.clone()
            },
            GitObjectContext {
                replacements_disabled: true,
                ..context.clone()
            },
        ] {
            assert_ne!(
                key,
                serde_json::to_string(&changed.key(&commit).unwrap()).unwrap()
            );
            assert_eq!(
                changed.portable(),
                changed.replacements.is_empty() && changed.grafts.is_empty()
            );
        }
        let sha256 = GitObjectContext {
            format: "sha256".into(),
            ..context
        };
        sha256.key(&commit).unwrap_err();
        sha256.key(&"a".repeat(64)).unwrap();
        GitObjectContext::default().key(&commit).unwrap_err();
    }

    #[test]
    fn shared_tree_lookup_retains_spelling_identity_and_modes() {
        let tree = HistoricalTree::new(vec![
            TreeEntry::parse("100755 blob one\tSrc/Lib.rs").unwrap(),
            TreeEntry::parse("120000 blob two\tlink").unwrap(),
        ]);
        assert_eq!(tree.paths(), ["Src/Lib.rs", "link"]);
        assert_eq!(tree.entries().len(), 2);
        assert_eq!(tree.entries().first().unwrap().path, "Src/Lib.rs");
        assert_eq!(tree.entry("Src/Lib.rs").unwrap().id, "one");
        assert!(tree.entry("Src/Lib.rs").unwrap().is_executable());
        assert!(tree.entry("link").unwrap().is_symlink());
        assert!(tree.entry("src/lib.rs").is_none());
    }
}
