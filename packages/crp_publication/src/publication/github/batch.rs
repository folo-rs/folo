use std::fmt::Write as _;

use ohno::AppError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::publication::binaries::Binary;
use crate::publication::github::Github;
use crate::publication::manifest::InvalidManifest;

/// Frozen tag-bound requests for one native target, linked to the parent manifest.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformBatch {
    pub schema_version: u32,
    pub publication_id: String,
    pub repository: String,
    pub target: String,
    pub binaries: Vec<Binary>,
    pub batch_id: String,
}

/// Version of the frozen native batch payload, covered by its existing content identity.
pub(crate) const PLATFORM_BATCH_SCHEMA_VERSION: u32 = 1;

impl PlatformBatch {
    /// Freezes the exact remaining work, including tag commits, independently of run attempts.
    pub fn seal(mut self) -> Result<Self, AppError> {
        self.batch_id = self.identity()?;
        Ok(self)
    }

    pub(crate) fn verify_identity(&self) -> Result<(), AppError> {
        if self.batch_id != self.identity()? {
            return Err(InvalidManifest::new(
                "platform batch identity does not match its contents".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    #[expect(
        clippy::unneeded_field_pattern,
        reason = "Exhaustive destructuring makes added identity fields a compile-time decision."
    )]
    fn identity(&self) -> Result<String, AppError> {
        // This tuple's exact representation is the persisted identity contract. Every
        // work-defining field participates; an exhaustive destructure forces an explicit
        // decision when the envelope grows. Do not reorder fields without compatibility review.
        let Self {
            schema_version,
            publication_id,
            repository,
            target,
            binaries,
            batch_id: _,
        } = self;
        let bytes =
            serde_json::to_vec(&(schema_version, publication_id, repository, target, binaries))?;
        let mut id = String::new();
        for byte in Sha256::digest(bytes) {
            write!(id, "{byte:02x}")?;
        }
        Ok(id)
    }
}

/// Matrix-routing facts only; the action release maps native triples to runners.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BatchArtifact {
    pub target: String,
    pub path: String,
    pub batch_id: String,
}

#[cfg_attr(test, mutants::skip)] // Remote tag recheck; local batch validation is unit-tested.
pub(crate) fn verify_batch_tags(
    batch: &PlatformBatch,
    output: &crate::PublicationOutput,
) -> Result<(), AppError> {
    let github = Github::new(&batch.repository, output)?;
    verify_tags(batch, |tag| github.tag(tag))
}

fn verify_tags(
    batch: &PlatformBatch,
    mut lookup: impl FnMut(&str) -> Result<Option<String>, AppError>,
) -> Result<(), AppError> {
    for binary in &batch.binaries {
        if lookup(&binary.tag)?.as_deref() != Some(binary.source_sha.as_str()) {
            return Err(InvalidManifest::new(format!(
                "tag {} no longer identifies the frozen binary source {}",
                binary.tag, binary.source_sha
            ))
            .into());
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn every_frozen_source_requires_a_matching_tag_observation() {
        let batch = PlatformBatch {
            schema_version: 1,
            publication_id: "publication".to_owned(),
            repository: "example/tools".to_owned(),
            target: "x86_64-unknown-linux-gnu".to_owned(),
            binaries: ["first", "second"]
                .into_iter()
                .map(|name| Binary {
                    name: name.to_owned(),
                    bin: name.to_owned(),
                    version: "1.0.0".to_owned(),
                    tag: format!("{name}-v1.0.0"),
                    source_sha: "a".repeat(40),
                })
                .collect(),
            batch_id: String::new(),
        }
        .seal()
        .unwrap();
        let mut observed = Vec::new();
        verify_tags(&batch, |tag| {
            observed.push(tag.to_owned());
            Ok(Some("a".repeat(40)))
        })
        .unwrap();
        assert_eq!(observed, ["first-v1.0.0", "second-v1.0.0"]);
        for source in [None, Some("b".repeat(40))] {
            let mut observed = Vec::new();
            verify_tags(&batch, |tag| {
                observed.push(tag.to_owned());
                Ok(source.clone())
            })
            .unwrap_err();
            assert_eq!(observed, ["first-v1.0.0"]);
        }
        verify_tags(&batch, |_| Err(UnavailableTag::new().into())).unwrap_err();
    }

    #[test]
    fn every_work_field_participates_in_batch_identity() {
        let value = serde_json::json!({
            "schema_version":1,"publication_id":"intent","repository":"example/tools",
            "target":"native","binaries":[],"batch_id":""
        });
        let original = serde_json::from_value::<PlatformBatch>(value.clone())
            .unwrap()
            .seal()
            .unwrap();
        for (field, replacement) in [
            ("schema_version", serde_json::json!(2)),
            ("publication_id", serde_json::json!("other")),
            ("repository", serde_json::json!("other/tools")),
            ("target", serde_json::json!("other")),
            (
                "binaries",
                serde_json::json!([{"name":"tool","bin":"tool","version":"1.0.0","tag":"tool-v1.0.0","source_sha":"a".repeat(40)}]),
            ),
        ] {
            let mut changed = value.clone();
            *changed.get_mut(field).unwrap() = replacement;
            let changed = serde_json::from_value::<PlatformBatch>(changed)
                .unwrap()
                .seal()
                .unwrap();
            assert_ne!(original.batch_id, changed.batch_id);
        }
    }

    #[ohno::error]
    struct UnavailableTag;
}
