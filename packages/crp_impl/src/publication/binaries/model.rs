use ohno::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::publication::manifest::immutable_commit;

/// An immutable release identity, independent of the platform that builds it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Binary {
    pub(crate) name: String,
    pub(crate) bin: String,
    pub(crate) version: String,
    pub(crate) tag: String,
    pub(crate) source_sha: String,
}

/// GitHub's upload state is needed to distinguish completed assets from failed uploads.
#[derive(Debug, Deserialize)]
pub struct Asset {
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

// Bound an individual package's native execution independently of workflow scheduling.
pub(crate) const ITEM_MINUTES: usize = 60;

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

impl Binary {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        let version = Version::parse(&self.version)?;
        if !identifier(&self.name)
            || !identifier(&self.bin)
            || version.to_string() != self.version
            || self.tag != format!("{}-v{}", self.name, self.version)
            || !immutable_commit(&self.source_sha)
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
}
