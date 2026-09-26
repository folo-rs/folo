//! Immutable publication intent transported between workflow jobs.

use std::fmt::Write as _;
use std::fs;
use std::io::Write;
use std::path::{Component, Path};

use crp_workspace::identity::immutable_commit;
use ohno::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::publication::candidate::package_identifier;
use crate::publication::config::{Configuration, NativeTarget};
use crate::{ReadFileError, WriteFileError};

/// Content-addressed intent; progress is recorded separately and never written into this file.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublicationManifest {
    pub id: String,
    pub publication: Publication,
}

impl PublicationManifest {
    pub fn new(publication: Publication) -> Result<Self, AppError> {
        publication.validate()?;
        Ok(Self {
            id: publication.identity()?,
            publication,
        })
    }

    pub fn read(path: &Path) -> Result<Self, AppError> {
        let contents = fs::read(path).map_err(|error| ReadFileError::caused_by(path, error))?;
        let manifest: Self = serde_json::from_slice(&contents).map_err(|error| {
            InvalidManifest::caused_by("cannot read publication manifest".to_owned(), error)
        })?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub(crate) fn validate(&self) -> Result<(), AppError> {
        self.publication.validate()?;
        if self.id != self.publication.identity()? {
            return Err(InvalidManifest::new(
                "publication identity does not match its content".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    /// Creates a manifest or confirms that a prior invocation wrote identical intent.
    pub fn write(&self, path: &Path) -> Result<(), AppError> {
        self.validate()?;
        if path
            .try_exists()
            .map_err(|error| ReadFileError::caused_by(path, error))?
        {
            if Self::read(path)?.id == self.id {
                return Ok(());
            }
            return Err(InvalidManifest::new(
                "publication output already contains different intent".to_owned(),
            )
            .into());
        }
        let parent = path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent).map_err(|error| WriteFileError::caused_by(path, error))?;
        let mut staging = NamedTempFile::new_in(parent)
            .map_err(|error| WriteFileError::caused_by(path, error))?;
        serde_json::to_writer_pretty(&mut staging, self)
            .map_err(|error| WriteFileError::caused_by(path, error))?;
        staging
            .write_all(b"\n")
            .map_err(|error| WriteFileError::caused_by(path, error))?;
        staging
            .persist_noclobber(path)
            .map_err(|error| WriteFileError::caused_by(path, error))?;
        Ok(())
    }
}

/// Frozen source and configured destinations, without any observed remote completion state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Publication {
    pub schema_version: u32,
    pub tool_version: String,
    pub source: String,
    pub workspace_manifest: String,
    pub config_path: String,
    pub configuration: Configuration,
    pub packages: Vec<Package>,
}

impl Publication {
    fn identity(&self) -> Result<String, AppError> {
        let bytes = serde_json::to_vec(self)?;
        let mut digest = String::new();
        for byte in Sha256::digest(&bytes) {
            write!(digest, "{byte:02x}")?;
        }
        Ok(digest)
    }

    fn validate(&self) -> Result<(), AppError> {
        if self.schema_version != PUBLICATION_SCHEMA_VERSION {
            return Err(InvalidManifest::new(format!(
                "unsupported publication schema {}",
                self.schema_version
            ))
            .into());
        }
        self.configuration.validate(Path::new(&self.config_path))?;
        if !immutable_commit(&self.source)
            || !relative_file(&self.workspace_manifest)
            || !relative_file(&self.config_path)
            || self.tool_version.is_empty()
        {
            return Err(InvalidManifest::new(
                "publication requires immutable sources and repository-relative input files"
                    .to_owned(),
            )
            .into());
        }
        let mut previous = None;
        for package in &self.packages {
            if previous.is_some_and(|name: &str| name >= package.name.as_str())
                || !relative_file(&package.manifest)
                || !package_identifier(&package.name)
                || semver::Version::parse(&package.version).is_err()
            {
                return Err(InvalidManifest::new(
                    "publication package requests must have unique sorted identities".to_owned(),
                )
                .into());
            }
            if let Some(binary) = &package.binary {
                let selected = self
                    .configuration
                    .binary_targets(&package.name, Some(&binary.targets))?;
                if !package_identifier(&binary.name) || selected != binary.targets {
                    return Err(InvalidManifest::new(
                        "publication binary request differs from configured targets".to_owned(),
                    )
                    .into());
                }
            }
            previous = Some(&package.name);
        }
        Ok(())
    }
}

/// Exact package request; its source is the manifest's immutable publication commit.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub manifest: String,
    pub binary: Option<Binary>,
}

/// Archive promises for the package's single default-feature executable.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binary {
    pub name: String,
    pub targets: Vec<NativeTarget>,
}

/// Publication transport evolves independently of version-plan/report schemas.
pub(crate) const PUBLICATION_SCHEMA_VERSION: u32 = 1;

fn relative_file(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(['\\', ':'])
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[ohno::error]
#[display("{reason}")]
pub(crate) struct InvalidManifest {
    reason: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;

    fn publication() -> Publication {
        Publication {
            schema_version: PUBLICATION_SCHEMA_VERSION,
            tool_version: "1.0.0".to_owned(),
            source: "a".repeat(40),
            workspace_manifest: "nested/Cargo.toml".to_owned(),
            config_path: "nested/.cargo/release_plan.toml".to_owned(),
            configuration: serde_json::from_value(json!({
                "schema-version": 1, "repository": "example/tools",
                "release-branch": "main", "targets": []
            }))
            .unwrap(),
            packages: vec![Package {
                name: "library".to_owned(),
                version: "1.0.0".to_owned(),
                manifest: "nested/library/Cargo.toml".to_owned(),
                binary: None,
            }],
        }
    }

    #[test]
    fn manifest_identity_is_stable_and_tracks_publication_intent() {
        let first = PublicationManifest::new(publication()).unwrap();
        let equivalent = PublicationManifest::new(publication()).unwrap();
        assert_eq!(first.id, equivalent.id);
        let mut changed = publication();
        changed.source = "c".repeat(40);
        assert_ne!(first.id, PublicationManifest::new(changed).unwrap().id);
        let mut changed = publication();
        changed.packages.first_mut().unwrap().version = "1.0.1".to_owned();
        assert_ne!(first.id, PublicationManifest::new(changed).unwrap().id);
    }

    #[test]
    fn accepts_only_full_commit_identities_and_portable_relative_files() {
        for commit in ["a".repeat(40), "b".repeat(64)] {
            assert!(immutable_commit(&commit));
        }
        for commit in ["HEAD", "main", "abc123", ""] {
            assert!(!immutable_commit(commit));
        }
        for path in ["Cargo.toml", "nested/.cargo/release_plan.toml"] {
            assert!(relative_file(path));
        }
        for path in [
            "",
            "/Cargo.toml",
            "../Cargo.toml",
            "C:/Cargo.toml",
            "nested\\Cargo.toml",
        ] {
            assert!(!relative_file(path));
        }
    }

    #[test]
    fn rejects_invalid_schema_source_and_package_requests() {
        let mut invalid = publication();
        invalid.schema_version = 2;
        PublicationManifest::new(invalid).unwrap_err();
        let mut invalid = publication();
        invalid.source = "main".to_owned();
        PublicationManifest::new(invalid).unwrap_err();
        let mut invalid = publication();
        invalid.packages.push(Package {
            name: "library".to_owned(),
            version: "1.0.1".to_owned(),
            manifest: "other/Cargo.toml".to_owned(),
            binary: None,
        });
        PublicationManifest::new(invalid).unwrap_err();
    }

    #[test]
    fn rejects_changed_envelope_identity_and_invalid_binary_names() {
        let mut envelope = PublicationManifest::new(publication()).unwrap();
        envelope.id = "not-the-content-digest".to_owned();
        envelope.validate().unwrap_err();

        let mut invalid = publication();
        invalid.configuration = serde_json::from_value(json!({
            "schema-version":1,"repository":"example/tools","release-branch":"main",
            "targets":["x86_64-unknown-linux-gnu"]
        }))
        .unwrap();
        invalid.packages.first_mut().unwrap().binary = Some(Binary {
            name: "../not-a-binary".to_owned(),
            targets: serde_json::from_value(json!(["x86_64-unknown-linux-gnu"])).unwrap(),
        });
        PublicationManifest::new(invalid).unwrap_err();
    }
}
