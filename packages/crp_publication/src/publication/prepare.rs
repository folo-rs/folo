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

// Native preparation composes Git/Cargo acquisition, candidate verification and persistence.
// publication_prepare and executable publication tests cover that composition; source admission,
// request capture and relative identity retain independent in-process coverage below.
#[cfg_attr(test, mutants::skip)]
pub fn prepare(
    manifest: &Path,
    config_path: Option<&Path>,
    source: &str,
    output: &Path,
    diagnostics: &PublicationOutput,
) -> Result<String, AppError> {
    let verbose = diagnostics.notes();
    require_source(source)?;
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

fn require_source(source: &str) -> Result<(), AppError> {
    if !immutable_commit(source) {
        return Err(
            InvalidManifest::new("source must be a full immutable commit ID".to_owned()).into(),
        );
    }
    Ok(())
}

// The candidate repository owns real tracked-path acquisition; preparation boundary tests
// verify it rejects dirty/untracked inputs. Serialization consumes those acquired paths below.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn capture_requests(
    repository: &Repository,
    requests: Vec<PackageRequest>,
    inputs: &[PathBuf],
) -> Result<(Vec<Package>, Vec<PathBuf>), AppError> {
    capture_requests_with(repository.root(), requests, inputs, |paths| {
        repository.require_tracked_paths(paths)
    })
}

fn capture_requests_with(
    root: &Path,
    requests: Vec<PackageRequest>,
    inputs: &[PathBuf],
    tracked: impl FnOnce(&[PathBuf]) -> Result<Vec<PathBuf>, AppError>,
) -> Result<(Vec<Package>, Vec<PathBuf>), AppError> {
    // One phase-local index observation covers both intent inputs and package manifests.
    // Preserve canonical input paths for later serialization without repeating membership work.
    let input_count = inputs.len();
    let paths: Vec<_> = inputs
        .iter()
        .cloned()
        .chain(requests.iter().map(|request| request.manifest.clone()))
        .collect();
    let mut inputs = tracked(&paths)?;
    let paths = inputs.split_off(input_count);
    debug_assert_eq!(requests.len(), paths.len());
    let packages = requests
        .into_iter()
        .zip(paths)
        .map(|(request, manifest)| {
            Ok(Package {
                name: request.name,
                version: request.version,
                manifest: repository_relative(root, &manifest)?,
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

// Preparation and release-context integrations redirect this configured fetch to owned local
// Git repositories and verify the resolved release-line identity, never a live remote.
#[cfg_attr(test, mutants::skip)]
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

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::publication::config::NativeTarget;
    use crate::publication::packages::BinaryRequest;

    #[test]
    fn preparation_requires_immutable_source_and_repository_relative_paths() {
        for source in ["a".repeat(40), "b".repeat(64)] {
            require_source(&source).unwrap();
        }
        for source in ["HEAD", "", "abc123"] {
            assert!(
                require_source(source)
                    .unwrap_err()
                    .find_source::<InvalidManifest>()
                    .is_some()
            );
        }
        assert_eq!(
            repository_relative(
                Path::new("repository"),
                Path::new("repository/tool/Cargo.toml")
            )
            .unwrap(),
            "tool/Cargo.toml"
        );
        assert!(
            repository_relative(Path::new("repository"), Path::new("another/Cargo.toml"),)
                .unwrap_err()
                .find_source::<InvalidManifest>()
                .is_some()
        );
    }

    #[test]
    fn request_capture_checks_all_inputs_once_and_uses_acquired_paths() {
        let inputs = [
            PathBuf::from("config"),
            PathBuf::from("manifest"),
            PathBuf::from("lock"),
        ];
        let requests = vec![
            PackageRequest {
                name: "library".into(),
                version: "1.2.3".into(),
                manifest: PathBuf::from("library-manifest"),
                binary: None,
            },
            PackageRequest {
                name: "tool".into(),
                version: "2.0.0".into(),
                manifest: PathBuf::from("tool-manifest"),
                binary: Some(BinaryRequest {
                    name: "executable".into(),
                    targets: vec![NativeTarget::LinuxX64],
                }),
            },
        ];
        let canonical: Vec<_> = [
            "repository/.cargo/release_plan.toml",
            "repository/Cargo.toml",
            "repository/Cargo.lock",
            "repository/library/Cargo.toml",
            "repository/tool/Cargo.toml",
        ]
        .map(PathBuf::from)
        .into_iter()
        .collect();
        let (packages, captured) =
            capture_requests_with(Path::new("repository"), requests, &inputs, |paths| {
                assert_eq!(
                    paths,
                    [
                        PathBuf::from("config"),
                        PathBuf::from("manifest"),
                        PathBuf::from("lock"),
                        PathBuf::from("library-manifest"),
                        PathBuf::from("tool-manifest"),
                    ]
                );
                Ok(canonical.clone())
            })
            .unwrap();
        assert_eq!(captured, canonical.get(..3).unwrap());
        assert_eq!(packages.len(), 2);
        let library = packages.first().unwrap();
        assert_eq!(
            (&*library.name, &*library.version, &*library.manifest),
            ("library", "1.2.3", "library/Cargo.toml")
        );
        assert!(library.binary.is_none());
        let tool = packages.last().unwrap();
        assert_eq!(
            (&*tool.name, &*tool.version, &*tool.manifest),
            ("tool", "2.0.0", "tool/Cargo.toml")
        );
        let binary = tool.binary.as_ref().unwrap();
        assert_eq!(binary.name, "executable");
        assert_eq!(binary.targets, [NativeTarget::LinuxX64]);
    }

    #[test]
    fn failed_tracked_input_acquisition_aborts_request_capture() {
        let error = capture_requests_with(Path::new("repository"), vec![], &[], |_| {
            Err(InvalidManifest::new("untracked input".to_owned()).into())
        })
        .unwrap_err();
        assert!(error.find_source::<InvalidManifest>().is_some());
    }
}
