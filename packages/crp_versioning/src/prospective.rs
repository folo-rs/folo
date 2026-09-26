// Disposable local workspaces isolate offline resolution from the live checkout.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::{fs, io};

use crp_diag::Verbose;
use crp_workspace::command::{run_capture, run_capture_input, run_capture_ok, run_capture_os};
use crp_workspace::metadata::{cargo_manifest_path, load_tracked_work_tree};
use ohno::AppError;

use crate::resolved::{Artifact, Inputs, canonical, relative};
use crate::{ReadFileError, WriteFileError, quote_path};

/// Owns a clone until it is discarded or retained for final compatibility evidence.
#[derive(Debug)]
pub struct Prospective {
    pub root: PathBuf,
    pub manifest: PathBuf,
    pub retained: bool,
}

/// Marker stored outside the candidate's tracked files to identify tool-owned workspaces.
pub const EVIDENCE_MARKER: &str = ".git/cargo-release-plan-preview";

impl Prospective {
    #[expect(
        clippy::create_dir,
        reason = "creation must fail when another invocation owns the path"
    )]
    pub fn new(output: &Path, inputs: &Inputs) -> Result<Self, AppError> {
        fs::create_dir_all(output).map_err(|error| WriteFileError::caused_by(output, error))?;
        let output = canonical(output)?;
        let root = output.join(".prospective");
        // Claim ownership atomically so a concurrent invocation cannot lose its clone to Drop.
        fs::create_dir(&root).map_err(|error| WriteFileError::caused_by(&root, error))?;
        let prospective = Self {
            manifest: root.join(&inputs.manifest),
            root,
            retained: false,
        };
        _ = run_capture_os(
            "git",
            [
                OsStr::new("clone"),
                OsStr::new("--quiet"),
                OsStr::new("--shared"),
                OsStr::new("--no-checkout"),
                OsStr::new("--"),
                inputs.root().as_os_str(),
                prospective.root.as_os_str(),
            ],
            inputs.root(),
        )?;
        for key in [
            "core.autocrlf",
            "core.eol",
            "core.filemode",
            "core.ignorecase",
        ] {
            if let Some(value) = run_capture_ok("git", &["config", "--get", key], inputs.root())? {
                _ = run_capture("git", &["config", key, value.trim()], &prospective.root)?;
            }
        }
        _ = run_capture(
            "git",
            &["update-ref", "HEAD", &inputs.head],
            &prospective.root,
        )?;
        // Tree objects omit intent-to-add entries. Reconstruct the captured index entries
        // directly so every tracked input participates in prospective classification.
        _ = run_capture("git", &["read-tree", "--empty"], &prospective.root)?;
        _ = run_capture_input(
            "git",
            &["update-index", "-z", "--index-info"],
            inputs.index().as_bytes(),
            &prospective.root,
        )?;
        for path in &inputs.paths {
            let source = inputs.root().join(path);
            if !source.exists() {
                continue;
            }
            let destination = prospective.root.join(path);
            if let Some(parent) = destination.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| WriteFileError::caused_by(parent, error))?;
            }
            fs::copy(&source, &destination)
                .map_err(|error| WriteFileError::caused_by(&destination, error))?;
        }
        Ok(prospective)
    }

    pub fn retain(mut self, output: &Path, owner: &Path) -> Result<PathBuf, AppError> {
        let manifest = relative(&self.root, &self.manifest)?;
        let destination = output.join("workspace");
        let owner = owner.to_string_lossy();
        if destination.exists() {
            let marker = destination.join(EVIDENCE_MARKER);
            if fs::read_to_string(&marker).ok().as_deref() != Some(owner.as_ref()) {
                return Err(EvidenceWorkspaceOccupied::new().into());
            }
            fs::remove_dir_all(&destination)
                .map_err(|error| WriteFileError::caused_by(&destination, error))?;
        }
        let marker = self.root.join(EVIDENCE_MARKER);
        fs::write(&marker, owner.as_bytes())
            .map_err(|error| WriteFileError::caused_by(&marker, error))?;
        fs::rename(&self.root, &destination)
            .map_err(|error| WriteFileError::caused_by(&destination, error))?;
        self.retained = true;
        Ok(destination.join(manifest))
    }

    // Cargo manifest acquisition and process execution require integration coverage.
    // The shared core retains the actual offline arguments and error propagation.
    #[cfg_attr(test, mutants::skip)]
    pub fn resolve(&self, verbose: Verbose<'_>) -> Result<(), AppError> {
        // Captured identity keeps the filesystem spelling; Cargo requires its conventional
        // manifest filename at the subprocess boundary even when another spelling aliases it.
        let manifest = cargo_manifest_path(&self.manifest);
        resolve_offline(
            &manifest,
            self.manifest.parent().expect("a manifest has a parent"),
            verbose,
            |args, root| {
                run_capture_os("cargo", args.iter().copied(), root)
                    .map(|_| ())
                    .map_err(Into::into)
            },
        )
    }

    // The Cargo workspace query is integration-only; capture_artifacts owns selection and bytes.
    #[cfg_attr(test, mutants::skip)]
    pub fn artifacts(&self, inputs: &Inputs) -> Result<Vec<Artifact>, AppError> {
        let (work_tree, _) = load_tracked_work_tree(&self.manifest)?;
        let mut paths: BTreeSet<PathBuf> = work_tree.member_manifests.into_iter().collect();
        paths.insert(work_tree.workspace_root.join("Cargo.toml"));
        paths.insert(work_tree.workspace_root.join("Cargo.lock"));
        capture_artifacts(&self.root, inputs.root(), paths, |path| {
            fs::read_to_string(path)
        })
    }
}

fn resolve_offline(
    manifest: &Path,
    root: &Path,
    verbose: Verbose<'_>,
    run: impl FnOnce(&[&OsStr], &Path) -> Result<(), AppError>,
) -> Result<(), AppError> {
    verbose.note(|| {
        format!(
            "resolving {} with cargo update --offline --workspace before release decisions; \
             existing third-party locks are retained where Cargo permits, but dependency edges \
             can be reselected and must be classified",
            quote_path(&manifest.to_string_lossy())
        )
    });
    run(
        &[
            OsStr::new("update"),
            OsStr::new("--offline"),
            OsStr::new("--workspace"),
            OsStr::new("--manifest-path"),
            manifest.as_os_str(),
        ],
        root,
    )
}

fn capture_artifacts(
    root: &Path,
    original: &Path,
    paths: BTreeSet<PathBuf>,
    mut read: impl FnMut(&Path) -> io::Result<String>,
) -> Result<Vec<Artifact>, AppError> {
    let mut artifacts = Vec::new();
    for path in paths {
        let relative = relative(root, &path)?;
        let contents = read(&path).map_err(|error| ReadFileError::caused_by(&path, error))?;
        if read(&original.join(&relative)).ok().as_ref() != Some(&contents) {
            artifacts.push(Artifact {
                path: relative,
                contents,
            });
        }
    }
    Ok(artifacts)
}

impl Drop for Prospective {
    fn drop(&mut self) {
        // A resolver error remains the useful diagnostic even if cleanup also fails.
        if !self.retained {
            _ = fs::remove_dir_all(&self.root);
        }
    }
}

/// A preview may replace only workspaces it previously created.
#[ohno::error]
#[display(
    "the retained workspace path is not owned by this preview; choose another output directory"
)]
pub(crate) struct EvidenceWorkspaceOccupied;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::io::ErrorKind;

    use super::*;
    use crate::resolved::UnsupportedInput;

    #[test]
    fn offline_resolution_preserves_manifest_arguments_working_directory_and_errors() {
        let manifest = Path::new("candidate/nested/Cargo.toml");
        let root = Path::new("candidate/nested");
        for fail in [false, true] {
            let mut called = false;
            let result = resolve_offline(
                manifest,
                root,
                Verbose::new(false, &crp_diag::Discard),
                |args, cwd| {
                    called = true;
                    assert_eq!(cwd, root);
                    assert_eq!(
                        args,
                        [
                            OsStr::new("update"),
                            OsStr::new("--offline"),
                            OsStr::new("--workspace"),
                            OsStr::new("--manifest-path"),
                            manifest.as_os_str(),
                        ]
                    );
                    if fail {
                        Err(ResolutionFailure::new().into())
                    } else {
                        Ok(())
                    }
                },
            );
            assert!(called);
            if fail {
                assert!(
                    result
                        .unwrap_err()
                        .find_source::<ResolutionFailure>()
                        .is_some()
                );
            } else {
                result.unwrap();
            }
        }
    }

    #[test]
    fn artifact_capture_emits_only_changed_or_unreadable_originals_with_exact_bytes() {
        let root = Path::new("candidate");
        let original = Path::new("original");
        let paths = ["Cargo.toml", "Cargo.lock", "member/Cargo.toml"].map(|path| root.join(path));
        for original_error in [ErrorKind::NotFound, ErrorKind::PermissionDenied] {
            let mut reads = Vec::new();
            let artifacts = capture_artifacts(root, original, paths.clone().into(), |path| {
                reads.push(path.to_owned());
                if path == root.join("Cargo.toml") || path == original.join("Cargo.toml") {
                    Ok("unchanged".to_owned())
                } else if path == root.join("Cargo.lock") {
                    Ok("resolved lockfile\n".to_owned())
                } else if path == original.join("Cargo.lock") {
                    Err(original_error.into())
                } else if path == root.join("member/Cargo.toml") {
                    Ok("new manifest\n".to_owned())
                } else {
                    assert_eq!(path, original.join("member/Cargo.toml"));
                    Ok("old manifest\n".to_owned())
                }
            })
            .unwrap();
            assert_eq!(
                artifacts,
                [
                    Artifact {
                        path: "Cargo.lock".into(),
                        contents: "resolved lockfile\n".to_owned()
                    },
                    Artifact {
                        path: "member/Cargo.toml".into(),
                        contents: "new manifest\n".to_owned()
                    },
                ]
            );
            assert_eq!(
                reads,
                [
                    "candidate/Cargo.lock",
                    "original/Cargo.lock",
                    "candidate/Cargo.toml",
                    "original/Cargo.toml",
                    "candidate/member/Cargo.toml",
                    "original/member/Cargo.toml"
                ]
                .map(PathBuf::from)
            );
        }
        assert!(
            capture_artifacts(root, original, paths.into(), |_| Ok("same".to_owned()))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn artifact_capture_propagates_candidate_read_and_rebasing_errors() {
        let root = Path::new("candidate");
        let path = root.join("Cargo.toml");
        let error = capture_artifacts(
            root,
            Path::new("original"),
            BTreeSet::from([path.clone()]),
            |requested| {
                assert_eq!(requested, path);
                Err(ErrorKind::PermissionDenied.into())
            },
        )
        .unwrap_err();
        assert!(error.find_source::<ReadFileError>().is_some());
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            ErrorKind::PermissionDenied
        );
        let error = capture_artifacts(
            root,
            Path::new("original"),
            BTreeSet::from([PathBuf::from("outside/Cargo.toml")]),
            |_| panic!(),
        )
        .unwrap_err();
        assert!(error.find_source::<UnsupportedInput>().is_some());
    }

    /// Records an offline process failure independently of its rendered diagnostic.
    #[ohno::error]
    struct ResolutionFailure;
}
