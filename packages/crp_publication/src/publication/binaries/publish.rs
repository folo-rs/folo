//! Unified publication adapter around the shared native batch engine.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use crp_native::command::install_cancellation_handler;
use ohno::AppError;
use serde::Serialize;

use crate::publication::binaries::batch::{Outcome, execute_items};
use crate::publication::binaries::{BinaryPublisher, Github};
use crate::publication::context::WorkflowRun;
use crate::publication::github::{PlatformBatch, verify_batch_tags};
use crate::publication::manifest::{InvalidManifest, PublicationManifest};
use crate::publication::registry::{verify_source, write_outcome};

/// The input identity and native item results from one target attempt.
#[derive(Debug, Serialize)]
struct BinaryOutcome {
    schema_version: u32,
    publication_id: String,
    phase: &'static str,
    target: String,
    no_upload: bool,
    complete: bool,
    batch_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    github: Option<WorkflowRun>,
    items: Vec<Outcome>,
}

#[cfg_attr(test, mutants::skip)] // Native execution/artifact wiring; validation and verdicts are unit-tested.
pub fn publish(
    publication_path: &Path,
    batch_path: &Path,
    manifest: &Path,
    output: &Path,
    artifacts: &Path,
    no_upload: bool,
) -> Result<(bool, String), AppError> {
    if output.try_exists()? || artifacts.try_exists()? {
        return Err(InvalidManifest::new(
            "binary publication requires new outcome and artifact destinations".to_owned(),
        )
        .into());
    }
    let github = WorkflowRun::capture()?;
    let publication = PublicationManifest::read(publication_path)?;
    let batch: PlatformBatch = serde_json::from_slice(&fs::read(batch_path)?)?;
    validate(&publication, &batch)?;
    verify_source(&publication, manifest)?;
    if !no_upload {
        verify_batch_tags(&batch)?;
    }
    install_cancellation_handler()?;
    let workspace = manifest
        .canonicalize()?
        .parent()
        .expect("a canonical manifest has a parent directory")
        .to_path_buf();
    let mut executor = BinaryPublisher::new(
        workspace,
        artifacts.to_path_buf(),
        batch.target.clone(),
        Github::new(batch.repository.clone()),
    )?;
    let items = execute_items(&batch.target, &batch.binaries, no_upload, &mut executor)?;
    let passed = successful(&items, batch.binaries.len(), no_upload);
    let outcome = BinaryOutcome {
        schema_version: 1,
        publication_id: publication.id,
        phase: "binaries",
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
            "Binary publication {} for {}; outcome: {}.",
            if passed { "completed" } else { "failed" },
            outcome.target,
            output.display()
        ),
    ))
}

fn successful(items: &[Outcome], expected: usize, no_upload: bool) -> bool {
    items.len() == expected
        && items.iter().all(|item| {
            item.cleanup_error.is_none()
                && if no_upload {
                    item.status == "staged-only"
                } else {
                    matches!(item.status, "published" | "skipped-complete")
                }
        })
}

fn validate(publication: &PublicationManifest, batch: &PlatformBatch) -> Result<(), AppError> {
    batch.verify_identity()?;
    if batch.schema_version != 1
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
                    status,
                    stage: "fixture",
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
