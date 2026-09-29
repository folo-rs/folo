//! Captures publication intent after validating a clean immutable release snapshot.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crp_workspace::command::run_capture;
use crp_workspace::git::GitRepo;
use crp_workspace::identity::immutable_commit;
use ohno::AppError;
use semver::Version;

use crate::PublicationOutput;
use crate::publication::candidate::{CandidateRequest, Repository, verify};
use crate::publication::config::Configuration;
use crate::publication::manifest::{
    Binary, InvalidManifest, PUBLICATION_SCHEMA_VERSION, Package, Publication, PublicationManifest,
};
use crate::publication::packages::{PackageRequest, PublicationWorkspace};

pub fn prepare(
    manifest: &Path,
    config_path: Option<&Path>,
    source: &str,
    output: &Path,
    diagnostics: &PublicationOutput,
) -> Result<String, AppError> {
    let verbose = diagnostics.notes();
    if !immutable_commit(source) {
        return Err(
            InvalidManifest::new("source must be a full immutable commit ID".to_owned()).into(),
        );
    }
    let repository = Repository::discover(manifest, source)?;
    repository.ensure_clean_head()?;
    let workspace = PublicationWorkspace::load(manifest)?;
    let (config_path, config) = Configuration::load(workspace.root(), config_path)?;
    verbose.note(|| format!(
        "Preparing publication from source {source} using {} for repository {} and release branch {}.",
        config_path.display(),
        config.repository(),
        config.release_branch()
    ));
    let (packages, inputs) = capture_requests(
        &repository,
        workspace.requests(&config)?,
        &[
            config_path,
            workspace.root().join("Cargo.toml"),
            workspace.root().join("Cargo.lock"),
        ],
    )?;
    let mut inputs = inputs.into_iter();
    let config_path = inputs
        .next()
        .expect("the first requested input is the configuration");
    let manifest = inputs
        .next()
        .expect("the second requested input is the workspace manifest");
    let line = fetch_release_line(repository.root(), &config)?;
    verbose.note(|| format!(
        "Fetched release branch at {line}; checking that source {source} belongs to its first-parent history."
    ));
    let requested: BTreeMap<_, _> = packages
        .iter()
        .map(|request| Ok((request.name.clone(), Version::parse(&request.version)?)))
        .collect::<Result<_, AppError>>()?;
    verify(
        &CandidateRequest {
            manifest_path: manifest.clone(),
            commit: source.to_owned(),
            release_line: line,
            packages: requested,
            verbose: verbose.enabled(),
        },
        verbose,
    )?;
    let publication = PublicationManifest::new(Publication {
        schema_version: PUBLICATION_SCHEMA_VERSION,
        tool_version: diagnostics.tool_version().to_owned(),
        source: source.to_owned(),
        workspace_manifest: repository_relative(repository.root(), &manifest)?,
        config_path: repository_relative(repository.root(), &config_path)?,
        configuration: config,
        packages,
    })?;
    // Detect source changes during input acquisition before persisting the captured intent.
    repository.ensure_clean_head()?;
    publication.write(output)?;
    Ok(format!(
        "Prepared publication {} from {source}: {}.",
        publication.id,
        output.display()
    ))
}

pub(crate) fn capture_requests(
    repository: &Repository,
    requests: Vec<PackageRequest>,
    inputs: &[PathBuf],
) -> Result<(Vec<Package>, Vec<PathBuf>), AppError> {
    // One phase-local index observation covers both intent inputs and package manifests.
    // Preserve canonical input paths for later serialization without repeating membership work.
    let input_count = inputs.len();
    let paths: Vec<_> = inputs
        .iter()
        .cloned()
        .chain(requests.iter().map(|request| request.manifest.clone()))
        .collect();
    let mut inputs = repository.require_tracked_paths(&paths)?;
    let paths = inputs.split_off(input_count);
    debug_assert_eq!(requests.len(), paths.len());
    let packages = requests
        .into_iter()
        .zip(paths)
        .map(|(request, manifest)| {
            Ok(Package {
                name: request.name,
                version: request.version,
                manifest: repository_relative(repository.root(), &manifest)?,
                binary: request.binary.map(|binary| Binary {
                    name: binary.name,
                    targets: binary.targets,
                }),
            })
        })
        .collect::<Result<_, AppError>>()?;
    Ok((packages, inputs))
}

fn repository_relative(root: &Path, path: &Path) -> Result<String, AppError> {
    let relative = path.strip_prefix(root).map_err(|error| {
        InvalidManifest::caused_by(
            "publication input is outside its repository".to_owned(),
            error,
        )
    })?;
    relative
        .to_str()
        .map(|path| path.replace('\\', "/"))
        .ok_or_else(|| InvalidManifest::new("publication paths must be UTF-8".to_owned()).into())
}

pub(crate) fn fetch_release_line(root: &Path, config: &Configuration) -> Result<String, AppError> {
    // This per-command helper uses the caller's GitHub authentication without modifying
    // global Git configuration or putting a credential into an argument.
    run_capture(
        "git",
        &[
            "-c",
            "credential.helper=",
            "-c",
            "credential.helper=!gh auth git-credential",
            "fetch",
            "--no-tags",
            &format!("https://github.com/{}.git", config.repository()),
            &format!("refs/heads/{}", config.release_branch()),
        ],
        root,
    )?;
    GitRepo::discover(root)?.rev_parse("FETCH_HEAD^{commit}")
}
