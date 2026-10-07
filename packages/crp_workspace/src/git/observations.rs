//! Immutable Git observations, separate from live ref and history-availability decisions.

use std::collections::HashMap;
use std::env::VarError;
use std::path::PathBuf;
use std::{env, fs, io};

use ohno::AppError;

use crate::ReadFileError;
use crate::command::{run_capture, run_capture_bytes};
use crate::git::{GitRepo, TreeEntry, path_text, strip_terminator};

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

    #[cfg_attr(test, mutants::skip)] // Native tree acquisition; shared lookup is tested in process.
    pub fn load(git: &GitRepo, commit: &str, context: &GitObjectContext) -> Result<Self, AppError> {
        context.validate_identity(commit)?;
        Ok(Self::new(git.ls_tree(commit, &[])?))
    }
}

/// Fresh interpretation inputs binding invocation memory to Git's effective object view.
///
/// Replacements and grafts participate in identity; parent availability stays a fresh query.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitObjectContext {
    format: String,
    replacements: String,
    grafts: Vec<u8>,
    replacement_base: Option<String>,
    replacements_disabled: bool,
}

impl GitObjectContext {
    #[cfg_attr(test, mutants::skip)] // Acquires Git/environment/filesystem interpretation inputs.
    pub fn capture(git: &GitRepo) -> Result<Self, AppError> {
        let replacement_base = replacement_namespace(env::var("GIT_REPLACE_REF_BASE"))?;
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
        let graft_path = run_capture_bytes(
            "git",
            &["rev-parse", "--git-path", "info/grafts"],
            git.root(),
        )?;
        let graft_path = git
            .root()
            .join(PathBuf::from(strip_terminator(path_text(&graft_path)?)));
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

    fn validate_identity(&self, commit: &str) -> Result<(), AppError> {
        // Sharing immutable observations requires resolved objects, not moving ref expressions.
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

fn replacement_namespace(value: Result<String, VarError>) -> Result<Option<String>, AppError> {
    match value {
        Ok(value) => Ok(Some(value)),
        Err(VarError::NotPresent) => Ok(None),
        Err(error @ VarError::NotUnicode(_)) => {
            Err(InvalidReplacementNamespace::caused_by(error).into())
        }
    }
}

/// Git inherits the exact namespace; substituting one changes object interpretation.
#[ohno::error]
#[display("GIT_REPLACE_REF_BASE cannot be represented as UTF-8")]
struct InvalidReplacementNamespace;

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
    ) -> Result<bool, AppError> {
        context.validate_identity(commit)?;
        self.parent_with(commit, || git.commit_has_parent_header(commit))
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

#[ohno::error]
#[display("immutable observations require a full resolved Git object identity: {commit}")]
struct UnresolvedObjectIdentity {
    commit: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn replacement_namespace_preserves_empty_and_custom_values_and_rejects_invalid_text() {
        assert_eq!(
            replacement_namespace(Err(VarError::NotPresent)).unwrap(),
            None
        );
        for value in ["", "refs/custom/"] {
            assert_eq!(
                replacement_namespace(Ok(value.into())).unwrap(),
                Some(value.into())
            );
        }
        assert!(
            replacement_namespace(Err(VarError::NotUnicode("unrepresentable".into())))
                .unwrap_err()
                .find_source::<InvalidReplacementNamespace>()
                .is_some()
        );
    }

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
    fn interpretation_changes_change_identity_and_only_full_object_names_are_admitted() {
        let context = GitObjectContext {
            format: "sha1".into(),
            ..GitObjectContext::default()
        };
        let commit = "a".repeat(40);
        context.validate_identity(&commit).unwrap();
        for invalid in ["HEAD", "main", "", "bad"] {
            assert!(
                context
                    .validate_identity(invalid)
                    .unwrap_err()
                    .find_source::<UnresolvedObjectIdentity>()
                    .is_some()
            );
        }
        context.validate_identity(&"z".repeat(40)).unwrap_err();
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
            assert_ne!(context, changed);
        }
        let sha256 = GitObjectContext {
            format: "sha256".into(),
            ..context
        };
        sha256.validate_identity(&commit).unwrap_err();
        sha256.validate_identity(&"a".repeat(64)).unwrap();
        GitObjectContext::default()
            .validate_identity(&commit)
            .unwrap_err();
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
