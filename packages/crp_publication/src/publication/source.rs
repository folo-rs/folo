use std::path::{Path, PathBuf};

use ohno::AppError;

use crate::publication::candidate::Repository;
use crate::publication::config::Configuration;
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::packages::PublicationWorkspace;
use crate::publication::prepare::capture_requests;

/// Source verification returns the root manifest already acquired with its workspace facts.
pub(crate) struct VerifiedSource {
    pub(crate) repository: Repository,
    pub(crate) manifest: PathBuf,
}

#[cfg_attr(test, mutants::skip)] // Git/Cargo/file acquisition is covered by publication boundaries.
pub(crate) fn verify_source(
    publication: &PublicationManifest,
    manifest: &Path,
) -> Result<VerifiedSource, AppError> {
    let repository = Repository::discover(manifest, &publication.publication.source)?;
    repository.ensure_clean_head()?;
    let workspace = PublicationWorkspace::load(manifest)?;
    let config_path = repository.root().join(&publication.publication.config_path);
    let (_, config) = Configuration::load(workspace.root(), Some(&config_path))?;
    let (requests, inputs) = capture_requests(
        &repository,
        workspace.requests(&config)?,
        &[
            workspace.root().join("Cargo.toml"),
            repository
                .root()
                .join(&publication.publication.workspace_manifest),
            config_path,
        ],
    )?;
    let mut inputs = inputs.into_iter();
    let workspace_manifest = inputs
        .next()
        .expect("the first requested input is the workspace manifest");
    let expected = inputs
        .next()
        .expect("the second requested input is the captured manifest");
    if workspace_manifest != expected {
        return Err(InvalidManifest::new(
            "selected workspace differs from publication intent".to_owned(),
        )
        .into());
    }
    if config != publication.publication.configuration {
        return Err(InvalidManifest::new(
            "source configuration differs from publication intent".to_owned(),
        )
        .into());
    }
    if requests != publication.publication.packages {
        return Err(InvalidManifest::new(
            "source packages differ from publication intent".to_owned(),
        )
        .into());
    }
    Ok(VerifiedSource {
        repository,
        manifest: workspace_manifest,
    })
}
