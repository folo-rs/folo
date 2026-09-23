use std::collections::BTreeSet;

use ohno::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};

/// Controller-discovered release requests and the repository's canonical target table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub(crate) targets: Vec<Target>,
    pub(crate) binaries: Vec<Request>,
}

/// An immutable release identity, independent of the platform that builds it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binary {
    pub(crate) name: String,
    pub(crate) bin: String,
    pub(crate) version: String,
    pub(crate) tag: String,
    pub(crate) source_sha: String,
}

/// Publication metadata adds target restrictions to an immutable binary identity.
#[derive(Debug, Deserialize)]
pub(crate) struct Request {
    #[serde(flatten)]
    pub(crate) binary: Binary,
    pub(crate) release_targets: Vec<String>,
}

/// A native target's runner assignment, supplied by the existing release policy.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) triple: String,
    pub(crate) os: String,
}

/// One platform job's frozen work, including its precomputed Actions timeout.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Batch {
    pub(crate) triple: String,
    pub(crate) os: String,
    pub(crate) timeout_minutes: usize,
    pub(crate) binaries: Vec<Binary>,
}

/// GitHub's upload state is needed to distinguish completed assets from failed uploads.
#[derive(Debug, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) state: String,
}

/// Asset inventory returned by `gh release view --json assets`.
#[derive(Debug, Deserialize)]
pub(crate) struct Release {
    pub(crate) assets: Vec<Asset>,
}

/// A malformed controller request cannot authorize publication.
#[ohno::error]
#[display("{message}")]
pub(crate) struct InvalidPlan {
    pub(crate) message: String,
}

// Preserve the release workflow's cold-setup and per-binary allowances, within Actions' ceiling.
const SETUP_MINUTES: usize = 90;
pub(crate) const ITEM_MINUTES: usize = 60;
const JOB_MINUTES: usize = 360;

pub(crate) fn timeout_minutes(items: usize) -> usize {
    // Saturation deliberately caps arbitrarily large batches at the hosted runner limit.
    SETUP_MINUTES
        .saturating_add(ITEM_MINUTES.saturating_mul(items))
        .min(JOB_MINUTES)
}

pub(crate) fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

pub(crate) fn runner_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

impl Binary {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        let version = Version::parse(&self.version)?;
        if !identifier(&self.name)
            || !identifier(&self.bin)
            || version.to_string() != self.version
            || self.tag != format!("{}-v{}", self.name, self.version)
            || self.source_sha.len() != 40
            || !self
                .source_sha
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(InvalidPlan::new(format!("Invalid release identity: {self:?}")).into());
        }
        Ok(())
    }

    pub(crate) fn archive_base(&self, triple: &str) -> String {
        format!("{}-{triple}", self.tag)
    }

    pub(crate) fn complete(&self, triple: &str, assets: &[Asset]) -> bool {
        let base = self.archive_base(triple);
        ["zip", "sha256"].iter().all(|extension| {
            let name = format!("{base}.{extension}");
            assets
                .iter()
                .any(|asset| asset.name == name && asset.state == "uploaded")
        })
    }
}

impl Batch {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if !identifier(&self.triple)
            || !runner_label(&self.os)
            || self.binaries.is_empty()
            || self.timeout_minutes != timeout_minutes(self.binaries.len())
        {
            return Err(InvalidPlan::new("Invalid platform batch".to_owned()).into());
        }
        let mut identities = BTreeSet::new();
        for binary in &self.binaries {
            binary.validate()?;
            if !identities.insert((&binary.name, &binary.version)) {
                return Err(InvalidPlan::new(format!("Duplicate release: {}", binary.tag)).into());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::indexing_slicing,
    reason = "Test fixtures specify every indexed item"
)]
pub(crate) mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Binary: Send, Sync, std::panic::UnwindSafe, std::panic::RefUnwindSafe);
    assert_impl_all!(Batch: Send, Sync, std::panic::UnwindSafe, std::panic::RefUnwindSafe);

    pub(crate) fn binary(name: &str) -> Binary {
        Binary {
            name: name.into(),
            bin: format!("{name}-bin"),
            version: "1.2.3".into(),
            tag: format!("{name}-v1.2.3"),
            source_sha: "a".repeat(40),
        }
    }

    #[test]
    fn preserves_names_versions_and_source_identity() {
        let binary = binary("tool");
        binary.validate().unwrap();
        let json = serde_json::to_string(&binary).unwrap();
        assert_eq!(serde_json::from_str::<Binary>(&json).unwrap(), binary);
        for field in ["name", "bin", "version", "tag", "source_sha"] {
            let mut value = serde_json::to_value(&binary).unwrap();
            value[field] = serde_json::Value::String("../invalid".into());
            assert!(
                serde_json::from_value::<Binary>(value)
                    .unwrap()
                    .validate()
                    .is_err()
            );
        }
    }

    #[test]
    fn controller_request_json_preserves_target_restrictions() {
        let mut request = serde_json::to_value(binary("tool")).unwrap();
        request["release_targets"] = serde_json::json!(["native"]);
        let plan: Plan = serde_json::from_value(serde_json::json!({
            "targets": [{"triple": "native", "os": "runner"}],
            "binaries": [request]
        }))
        .unwrap();
        assert_eq!(plan.binaries[0].binary, binary("tool"));
        assert_eq!(plan.binaries[0].release_targets, ["native"]);
    }

    #[test]
    fn both_assets_must_be_uploaded() {
        let binary = binary("tool");
        let archive = format!("{}.zip", binary.archive_base("native"));
        let checksum = format!("{}.sha256", binary.archive_base("native"));
        for archive_state in ["uploaded", "starter"] {
            for checksum_state in ["uploaded", "starter"] {
                let assets = vec![
                    Asset {
                        name: archive.clone(),
                        state: archive_state.into(),
                    },
                    Asset {
                        name: checksum.clone(),
                        state: checksum_state.into(),
                    },
                ];
                assert_eq!(
                    binary.complete("native", &assets),
                    archive_state == "uploaded" && checksum_state == "uploaded"
                );
                assert!(!binary.complete("native", &assets[..1]));
                assert!(!binary.complete("different", &assets));
            }
        }
        assert!(!binary.complete("native", &[]));
    }

    #[test]
    fn timeout_retains_existing_allowance_and_caps_large_batches() {
        assert_eq!(timeout_minutes(1), 150);
        assert_eq!(timeout_minutes(3), 270);
        assert_eq!(timeout_minutes(usize::MAX), 360);
    }

    #[test]
    fn batch_rejects_duplicates_empty_and_invalid_budget() {
        let mut batch = Batch {
            triple: "native".into(),
            os: "runner".into(),
            timeout_minutes: 150,
            binaries: vec![binary("tool")],
        };
        batch.validate().unwrap();
        batch.binaries.push(binary("tool"));
        batch.timeout_minutes = timeout_minutes(2);
        assert!(batch.validate().is_err());
        batch.binaries.clear();
        batch.timeout_minutes = timeout_minutes(0);
        assert!(batch.validate().is_err());
    }
}
