use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crp_diag::Verbose;
use crp_versioning::classify::{PackageStatus, classify};
use crp_workspace::command::run_capture;
use crp_workspace::git::GitRepo;
use ohno::AppError;
use tempfile::TempDir;

use crate::PublicationOutput;
use crate::publication::manifest::{InvalidManifest, Package, PublicationManifest};
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::fetch_release_line;

/// A disposable fetched release tip with one reusable source classification.
pub(crate) struct Candidate {
    directory: Option<TempDir>,
    repository: PathBuf,
    root: PathBuf,
    pub(crate) source: String,
    pub(crate) packages: BTreeMap<String, CandidatePackage>,
    diagnostics: PublicationOutput,
}

impl Candidate {
    #[cfg_attr(test, mutants::skip)] // Git worktree ownership and source acquisition.
    pub(crate) fn create(
        repository: &Path,
        publication: &PublicationManifest,
        diagnostics: &PublicationOutput,
    ) -> Result<Self, AppError> {
        let source = fetch_release_line(repository, &publication.publication.configuration)?;
        if !GitRepo::discover(repository)?
            .first_parent_commits(&source)?
            .contains(&publication.publication.source)
        {
            return Err(InvalidManifest::new(
                "release candidate no longer descends from the original publication source"
                    .to_owned(),
            )
            .into());
        }
        let directory = tempfile::Builder::new()
            .prefix("cargo-release-plan-candidate-")
            .tempdir()?;
        let root = directory.path().join("source");
        let candidate = Self {
            directory: Some(directory),
            repository: repository.to_path_buf(),
            root,
            source,
            packages: BTreeMap::new(),
            diagnostics: diagnostics.clone(),
        };
        run_capture(
            "git",
            &[
                "worktree",
                "add",
                "--detach",
                &candidate.root.to_string_lossy(),
                &candidate.source,
            ],
            repository,
        )?;
        candidate.classify(publication, diagnostics.notes())
    }

    #[cfg_attr(test, mutants::skip)] // Real Git/Cargo classification supplies the pure eligibility facts.
    fn classify(
        mut self,
        publication: &PublicationManifest,
        verbose: Verbose<'_>,
    ) -> Result<Self, AppError> {
        let manifest = self.root.join(&publication.publication.workspace_manifest);
        let result = classify(&manifest, Some(&publication.publication.source), verbose);
        match result {
            Ok(classification) => {
                for package in classification.packages {
                    let unchanged = package.status() == PackageStatus::Unchanged;
                    self.packages.insert(
                        package.name,
                        CandidatePackage {
                            version: package.declared_version.to_string(),
                            unchanged,
                        },
                    );
                }
                Ok(self)
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn supports(&self, package: &Package) -> bool {
        self.packages
            .get(&package.name)
            .is_some_and(|candidate| candidate.unchanged && candidate.version == package.version)
    }

    #[cfg_attr(test, mutants::skip)] // Owned Git and filesystem cleanup.
    pub(crate) fn finish(&mut self) -> Result<(), AppError> {
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
        let cleanup = directory.close();
        if let Err(error) = &cleanup {
            self.diagnostics
                .line(format_args!("Candidate directory cleanup failed: {error}"));
        }
        git?;
        cleanup?;
        Ok(())
    }
}

impl Drop for Candidate {
    #[cfg_attr(test, mutants::skip)] // Fallback for native worktree cleanup on early returns.
    fn drop(&mut self) {
        // Cleanup is also needed after classification failure; failures remain visible.
        if let Err(error) = self.finish() {
            self.diagnostics
                .line(format_args!("Candidate worktree cleanup failed: {error}"));
        }
    }
}

/// Retains the observed version even when it cannot satisfy an older tag request.
pub(crate) struct CandidatePackage {
    pub(crate) version: String,
    unchanged: bool,
}

#[cfg_attr(test, mutants::skip)] // Native historical checkout; identity predicate is unit-tested.
pub(crate) fn verify_tag_source(
    root: &Path,
    publication: &PublicationManifest,
    package: &Package,
    source: &str,
    diagnostics: &PublicationOutput,
) -> Result<(), AppError> {
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
    let directory = tempfile::Builder::new()
        .prefix("cargo-release-plan-tag-")
        .tempdir()?;
    let mut checkout = Candidate {
        root: directory.path().join("source"),
        directory: Some(directory),
        repository: root.to_path_buf(),
        source: source.to_owned(),
        packages: BTreeMap::new(),
        diagnostics: diagnostics.clone(),
    };
    run_capture(
        "git",
        &[
            "worktree",
            "add",
            "--detach",
            &checkout.root.to_string_lossy(),
            source,
        ],
        root,
    )?;
    let result = PublicationWorkspace::load(
        &checkout
            .root
            .join(&publication.publication.workspace_manifest),
    )
    .and_then(|workspace| {
        if workspace.contains_release(
            &package.name,
            &package.version,
            package.binary.as_ref().map(|binary| binary.name.as_str()),
        ) {
            Ok(())
        } else {
            Err(InvalidManifest::new(format!(
                "tag source {source} does not contain {}@{}",
                package.name, package.version
            ))
            .into())
        }
    });
    checkout.finish()?;
    result
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn candidate(version: &str, unchanged: bool) -> Candidate {
        // Acquired observations remain meaningful after their worktree has been released.
        Candidate {
            diagnostics: PublicationOutput::new(
                "1.2.3",
                false,
                std::sync::Arc::new(crp_diag::Discard),
            ),
            directory: None,
            repository: PathBuf::new(),
            root: PathBuf::new(),
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
