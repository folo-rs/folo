//! Captures publication intent after validating a clean immutable release snapshot.

use std::collections::BTreeMap;
use std::path::Path;

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
    let config_path = repository.require_tracked(&config_path)?;
    let manifest = repository.require_tracked(&workspace.root().join("Cargo.toml"))?;
    _ = repository.require_tracked(&workspace.root().join("Cargo.lock"))?;
    let requests = workspace.requests(&config)?;
    let line = fetch_release_line(repository.root(), &config)?;
    verbose.note(|| format!(
        "Fetched release branch at {line}; checking that source {source} belongs to its first-parent history."
    ));
    let requested: BTreeMap<_, _> = requests
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
    repository.checked(|| {
        // Full-graph validation enforces the committed resolution; no repair is allowed here.
        verbose.note(|| "Validating the complete committed Cargo.lock with locked metadata before capturing publication requests.".to_owned());
        run_capture(
            "cargo",
            &[
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
                &manifest.to_string_lossy(),
            ],
            workspace.root(),
        )
        .map_err(Into::into)
    })?;
    let packages = capture_requests(&repository, requests)?;
    let publication = PublicationManifest::new(Publication {
        schema_version: PUBLICATION_SCHEMA_VERSION,
        tool_version: diagnostics.tool_version().to_owned(),
        source: source.to_owned(),
        workspace_manifest: tracked_relative(&repository, &manifest)?,
        config_path: tracked_relative(&repository, &config_path)?,
        configuration: config,
        packages,
    })?;
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
) -> Result<Vec<Package>, AppError> {
    requests
        .into_iter()
        .map(|request| {
            Ok(Package {
                name: request.name,
                version: request.version,
                manifest: tracked_relative(repository, &request.manifest)?,
                binary: request.binary.map(|binary| Binary {
                    name: binary.name,
                    targets: binary.targets,
                }),
            })
        })
        .collect()
}

fn tracked_relative(repository: &Repository, path: &Path) -> Result<String, AppError> {
    let path = repository.require_tracked(path)?;
    let relative = path.strip_prefix(repository.root()).map_err(|error| {
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
