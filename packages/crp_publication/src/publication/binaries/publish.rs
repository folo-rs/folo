//! Unified publication adapter around the shared native batch engine.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use crp_native::command::install_cancellation_handler;
use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::publication::binaries::batch::{Outcome, execute_items};
use crate::publication::binaries::{BinaryPublisher, Github};
use crate::publication::context::WorkflowRun;
use crate::publication::github::{PLATFORM_BATCH_SCHEMA_VERSION, PlatformBatch, verify_batch_tags};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::registry::{verify_source, write_outcome};
use crate::{PublicationOutput, ReadFileError};

/// The input identity and native item results from one target attempt.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BinaryReceipt {
    pub(crate) schema_version: u32,
    pub(crate) publication_id: String,
    pub(crate) phase: String,
    pub(crate) target: String,
    pub(crate) no_upload: bool,
    pub(crate) complete: bool,
    pub(crate) batch_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) github: Option<WorkflowRun>,
    pub(crate) items: Vec<Outcome>,
}

/// Binary outcome format shared by the producer and phase-specific evidence reader.
pub(crate) const BINARY_OUTCOME_SCHEMA_VERSION: u32 = 1;

#[cfg_attr(test, mutants::skip)] // Native execution/artifact wiring; validation and verdicts are unit-tested.
pub fn publish(
    publication_path: &Path,
    batch_path: &Path,
    manifest: &Path,
    output: &Path,
    artifacts: &Path,
    no_upload: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    for path in [output, artifacts] {
        if path
            .try_exists()
            .map_err(|error| ReadFileError::caused_by(path, error))?
        {
            return Err(BinaryDestinationExists::new(path).into());
        }
    }
    let github = WorkflowRun::capture()?;
    let publication = PublicationManifest::read(publication_path)?;
    let bytes =
        fs::read(batch_path).map_err(|error| ReadFileError::caused_by(batch_path, error))?;
    let batch = decode_batch(&bytes, batch_path)?;
    validate(&publication, &batch)
        .map_err(|error| BinaryBatchInputError::caused_by(batch_path, error))?;
    verify_source(&publication, manifest)?;
    if !no_upload {
        verify_batch_tags(&batch, diagnostics)?;
    }
    install_cancellation_handler()?;
    let workspace = manifest
        .canonicalize()
        .map_err(|error| ReadFileError::caused_by(manifest, error))?
        .parent()
        .expect("a canonical manifest has a parent directory")
        .to_path_buf();
    let mut executor = BinaryPublisher::new(
        workspace,
        artifacts.to_path_buf(),
        batch.target.clone(),
        Github::new(batch.repository.clone(), diagnostics.clone()),
    )?;
    let items = execute_items(
        &batch.target,
        &batch.binaries,
        no_upload,
        &mut executor,
        diagnostics,
    )?;
    let passed = successful(&items, batch.binaries.len(), no_upload);
    let outcome = BinaryReceipt {
        schema_version: BINARY_OUTCOME_SCHEMA_VERSION,
        publication_id: publication.id,
        phase: "binaries".to_owned(),
        target: batch.target,
        no_upload,
        complete: passed && !no_upload,
        batch_id: batch.batch_id,
        github,
        items,
    };
    write_outcome(output, &outcome)?;
    Ok((
        passed,
        format!(
            "Binary {} {} for {}; outcome: {}.",
            if no_upload { "staging" } else { "publication" },
            if passed { "completed" } else { "failed" },
            outcome.target,
            output.display()
        ),
    ))
}

fn decode_batch(bytes: &[u8], path: &Path) -> Result<PlatformBatch, AppError> {
    serde_json::from_slice(bytes)
        .map_err(|error| BinaryBatchInputError::caused_by(path, error).into())
}

/// Identifies the frozen batch that could not be decoded or validated.
#[ohno::error]
#[display("cannot read binary publication batch {}", path.display())]
struct BinaryBatchInputError {
    path: PathBuf,
}

/// A target attempt must not overwrite an existing outcome or archive destination.
#[ohno::error]
#[display("binary publication destination must be new: {}", path.display())]
struct BinaryDestinationExists {
    path: PathBuf,
}

fn successful(items: &[Outcome], expected: usize, no_upload: bool) -> bool {
    items.len() == expected
        && items.iter().all(|item| {
            item.cleanup_error.is_none()
                && if no_upload {
                    item.status == "staged-only"
                } else {
                    matches!(item.status.as_str(), "published" | "skipped-complete")
                }
        })
}

fn validate(publication: &PublicationManifest, batch: &PlatformBatch) -> Result<(), AppError> {
    batch.verify_identity()?;
    if batch.schema_version != PLATFORM_BATCH_SCHEMA_VERSION
        || batch.publication_id != publication.id
        || batch.repository != publication.publication.configuration.repository()
        || batch.binaries.is_empty()
    {
        return Err(InvalidManifest::new(
            "binary batch does not match publication intent".to_owned(),
        )
        .into());
    }
    let mut identities = BTreeSet::new();
    for binary in &batch.binaries {
        binary.validate()?;
        let Some(package) = publication
            .publication
            .packages
            .iter()
            .find(|package| package.name == binary.name && package.version == binary.version)
        else {
            return Err(InvalidManifest::new(
                "binary batch contains an unrequested package version".to_owned(),
            )
            .into());
        };
        if !identities.insert(&binary.name)
            || package.binary.as_ref().is_none_or(|request| {
                request.name != binary.bin
                    || !request
                        .targets
                        .iter()
                        .any(|target| target.triple() == batch.target)
            })
        {
            return Err(InvalidManifest::new(
                "binary batch changes the executable or selected target".to_owned(),
            )
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::slice;

    use serde_json::json;

    use super::*;

    #[test]
    fn malformed_batch_diagnostics_retain_the_input_path_and_cause() {
        let path = Path::new("retained/batch.json");
        let error = decode_batch(b"{", path).unwrap_err();
        assert_eq!(
            error.find_source::<BinaryBatchInputError>().unwrap().path,
            path
        );
        assert!(error.find_source::<serde_json::Error>().is_some());
    }

    fn publication() -> PublicationManifest {
        PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main",
                "targets":["x86_64-unknown-linux-gnu"]},
            "packages":[{"name":"tool","version":"1.0.0","manifest":"Cargo.toml",
                "binary":{"name":"tool-bin","targets":["x86_64-unknown-linux-gnu"]}}]
        })).unwrap()).unwrap()
    }

    fn batch(publication: &PublicationManifest) -> PlatformBatch {
        serde_json::from_value::<PlatformBatch>(json!({
            "schema_version":1,"publication_id":publication.id,"repository":"example/tool",
            "target":"x86_64-unknown-linux-gnu","batch_id":"","binaries":[{
                "name":"tool","version":"1.0.0","bin":"tool-bin","tag":"tool-v1.0.0","source_sha":"b".repeat(40)
            }]
        })).unwrap()
        .seal()
        .unwrap()
    }

    #[test]
    fn validates_manifest_linkage_without_conflating_tag_and_publication_sources() {
        let publication = publication();
        validate(&publication, &batch(&publication)).unwrap();
        let mut invalid = batch(&publication);
        invalid.publication_id = "different".to_owned();
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.schema_version = 2;
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.repository = "example/another".to_owned();
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.target = "x86_64-pc-windows-msvc".to_owned();
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.binaries.first_mut().unwrap().bin = "another".to_owned();
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid
            .binaries
            .push(invalid.binaries.first().unwrap().clone());
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.binaries.clear();
        validate(&publication, &invalid.seal().unwrap()).unwrap_err();
        let mut invalid = batch(&publication);
        invalid.batch_id = "tampered".to_owned();
        validate(&publication, &invalid).unwrap_err();
    }

    #[test]
    fn valid_binary_identities_must_name_a_requested_package_version() {
        let publication = publication();
        for (name, version) in [("another", "1.0.0"), ("tool", "2.0.0")] {
            let mut invalid = batch(&publication);
            let binary = invalid.binaries.first_mut().unwrap();
            binary.name = name.to_owned();
            binary.version = version.to_owned();
            binary.tag = format!("{name}-v{version}");
            binary.validate().unwrap();
            let error = validate(&publication, &invalid.seal().unwrap()).unwrap_err();
            assert!(error.find_source::<InvalidManifest>().is_some());
        }
    }

    #[test]
    fn only_mode_appropriate_complete_items_pass_the_attempt() {
        let publication = publication();
        let binary = batch(&publication).binaries.into_iter().next().unwrap();
        for status in [
            "published",
            "skipped-complete",
            "staged-only",
            "failed",
            "unattempted",
            "unknown",
        ] {
            for no_upload in [false, true] {
                let mut item = Outcome {
                    binary: binary.clone(),
                    status: status.to_owned(),
                    stage: "fixture".to_owned(),
                    diagnostic: None,
                    cleanup_error: None,
                };
                let expected = if no_upload {
                    status == "staged-only"
                } else {
                    matches!(status, "published" | "skipped-complete")
                };
                assert_eq!(successful(slice::from_ref(&item), 1, no_upload), expected);
                assert!(!successful(slice::from_ref(&item), 2, no_upload));
                item.cleanup_error = Some("failed".to_owned());
                assert!(!successful(slice::from_ref(&item), 1, no_upload));
            }
        }
    }
}
