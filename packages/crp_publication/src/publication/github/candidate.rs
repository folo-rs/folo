use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crp_versioning::classify::{PackageStatus, SnapshotCache, classify_with_cache};
use crp_workspace::command::run_capture;
use crp_workspace::git::GitRepo;
use ohno::AppError;
use tempfile::TempDir;

use crate::publication::manifest::{InvalidManifest, Package, PublicationManifest};
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::fetch_release_line;
use crate::{PublicationOutput, WriteFileError};

/// Owned release-branch-tip facts used to select a release-equivalent tag target.
pub(crate) struct Candidate {
    pub(crate) source: String,
    pub(crate) packages: BTreeMap<String, CandidatePackage>,
}

impl Candidate {
    #[cfg_attr(test, mutants::skip)] // Git worktree ownership and source acquisition.
    pub(crate) fn create(
        repository: &Path,
        publication: &PublicationManifest,
        diagnostics: &PublicationOutput,
    ) -> Result<Self, AppError> {
        let source = fetch_release_line(repository, &publication.publication.configuration)?;
        // Stop at the original source's ancestry rather than enumerating the repository's
        // lifetime. Only that exact first-parent boundary establishes membership.
        let original = &publication.publication.source;
        let boundary = format!("-{original}");
        if source != *original
            && !run_capture(
                "git",
                &[
                    "rev-list",
                    "--first-parent",
                    "--boundary",
                    &format!("{original}..{source}"),
                ],
                repository,
            )?
            .lines()
            .any(|line| line == boundary)
        {
            return Err(InvalidManifest::new(
                "release-branch tip no longer descends from the original publication source"
                    .to_owned(),
            )
            .into());
        }
        let packages = Worktree::observe(repository, &source, diagnostics, |root| {
            let manifest = root.join(&publication.publication.workspace_manifest);
            // This disposable publication checkout has no caller-selected observation store.
            // Keep only invocation memory rather than creating cache files inside the candidate.
            let classification = classify_with_cache(
                &manifest,
                Some(original),
                None,
                diagnostics.notes(),
                &mut SnapshotCache::default(),
            )?;
            Ok(classification
                .packages
                .into_iter()
                .map(|package| {
                    let unchanged = package.status() == PackageStatus::Unchanged;
                    (
                        package.name,
                        CandidatePackage {
                            version: package.declared_version.to_string(),
                            unchanged,
                        },
                    )
                })
                .collect())
        })?;
        Ok(Self { source, packages })
    }

    pub(crate) fn supports(&self, package: &Package) -> bool {
        self.packages
            .get(&package.name)
            .is_some_and(|candidate| candidate.unchanged && candidate.version == package.version)
    }
}

/// Owns only acquisition lifetime; all returned observations outlive the disposable checkout.
struct Worktree {
    directory: Option<TempDir>,
    repository: PathBuf,
    root: PathBuf,
    diagnostics: PublicationOutput,
}

impl Worktree {
    #[cfg_attr(test, mutants::skip)] // Real checkout acquisition and owned cleanup.
    fn observe<T>(
        repository: &Path,
        source: &str,
        diagnostics: &PublicationOutput,
        observe: impl FnOnce(&Path) -> Result<T, AppError>,
    ) -> Result<T, AppError> {
        let directory = tempfile::Builder::new()
            .prefix("cargo-release-plan-source-")
            .tempdir()?;
        let mut checkout = Self {
            root: directory.path().join("source"),
            directory: Some(directory),
            repository: repository.to_path_buf(),
            diagnostics: diagnostics.clone(),
        };
        let result = run_capture(
            "git",
            &[
                "worktree",
                "add",
                "--detach",
                &checkout.root.to_string_lossy(),
                source,
            ],
            repository,
        )
        .map_err(AppError::from)
        .and_then(|_| observe(&checkout.root));
        let cleanup = checkout.finish();
        match (result, cleanup) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), Ok(())) | (Ok(_), Err(error)) => Err(error),
            (Err(error), Err(cleanup)) => {
                Err(SourceAndCleanupFailed::caused_by(cleanup, error).into())
            }
        }
    }

    #[cfg_attr(test, mutants::skip)] // Owned Git and filesystem cleanup.
    fn finish(&mut self) -> Result<(), AppError> {
        let Some(directory) = self.directory.take() else {
            return Ok(());
        };
        let git = run_capture(
            "git",
            &[
                "worktree",
                "remove",
                "--force",
                &self.root.to_string_lossy(),
            ],
            &self.repository,
        );
        let cleanup = directory
            .close()
            .map_err(|error| WriteFileError::caused_by(&self.root, error));
        match (git, cleanup) {
            (Ok(_), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error.into()),
            (Ok(_), Err(error)) => Err(error.into()),
            (Err(error), Err(cleanup)) => {
                Err(SourceAndCleanupFailed::caused_by(cleanup, error).into())
            }
        }
    }
}

impl Drop for Worktree {
    #[cfg_attr(test, mutants::skip)] // Fallback for real Git worktree cleanup on early returns.
    fn drop(&mut self) {
        // Cleanup is also needed after classification failure; failures remain visible.
        if let Err(error) = self.finish() {
            self.diagnostics
                .best_effort_line(format_args!("Candidate worktree cleanup failed: {error}"));
        }
    }
}

/// Retains the observed version even when it cannot satisfy an older tag request.
pub(crate) struct CandidatePackage {
    pub(crate) version: String,
    unchanged: bool,
}

/// Both the original source operation and owned cleanup failed.
#[ohno::error]
#[display("source operation failed; cleanup also failed: {cleanup}")]
struct SourceAndCleanupFailed {
    cleanup: AppError,
}

#[cfg_attr(test, mutants::skip)] // Real historical checkout; identity predicate is unit-tested.
pub(crate) fn tag_workspace(
    root: &Path,
    publication: &PublicationManifest,
    source: &str,
    diagnostics: &PublicationOutput,
) -> Result<PublicationWorkspace, AppError> {
    let git = GitRepo::discover(root)?;
    if git.rev_parse(&format!("{source}^{{commit}}")).is_err() {
        run_capture(
            "git",
            &[
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=!gh auth git-credential",
                "fetch",
                "--no-tags",
                &format!(
                    "https://github.com/{}.git",
                    publication.publication.configuration.repository()
                ),
                source,
            ],
            root,
        )?;
    }
    Worktree::observe(root, source, diagnostics, |root| {
        PublicationWorkspace::load(&root.join(&publication.publication.workspace_manifest))
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn candidate(version: &str, unchanged: bool) -> Candidate {
        // Acquired observations remain meaningful after their worktree has been released.
        Candidate {
            source: "b".repeat(40),
            packages: BTreeMap::from([(
                "tool".to_owned(),
                CandidatePackage {
                    version: version.to_owned(),
                    unchanged,
                },
            )]),
        }
    }
}
