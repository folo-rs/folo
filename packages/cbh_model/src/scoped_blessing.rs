//! Project-level blessing records and keys, separate from concrete run partitions.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{BlessingRecord, BlessingScope, OBJECTS_SEGMENT, STORAGE_VERSION, sanitize_segment};

/// A blessing whose scope persists independently of stored measurements.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ScopedBlessingRecord {
    /// Benchmark selection, anchor, and issuance provenance.
    #[serde(flatten)]
    pub record: BlessingRecord,
    /// Required on read: an absent or misspelled scope must not accept everything.
    pub scope: BlessingScope,
}

impl ScopedBlessingRecord {
    /// The immutable object key for this issuance.
    ///
    /// Nanosecond issue times distinguish separate invocations; a key collision
    /// must be reported by write-once storage, never overwrite another acceptance.
    #[must_use]
    pub fn key(&self, project: &str) -> String {
        let project = sanitize_segment(project);
        let commit = sanitize_segment(&self.record.commit);
        let issued = self.record.issued_at.as_nanosecond();
        format!(
            "{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/blessings/{commit}/bless-{issued}.json"
        )
    }

    /// Encodes a scoped blessing for storage.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Decodes a scoped blessing, requiring explicit scope axes.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed records.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Extracts the anchor commit from a project-level blessing key.
#[must_use]
pub fn parse_scoped_blessing_key(key: &str) -> Option<&str> {
    let mut parts = key.split('/');
    if parts.next()? != STORAGE_VERSION {
        return None;
    }
    let project = parts.next()?;
    if project.is_empty() || parts.next()? != OBJECTS_SEGMENT || parts.next()? != "blessings" {
        return None;
    }
    let commit = parts.next()?;
    let file = parts.next()?;
    if commit.is_empty() || parts.next().is_some() {
        return None;
    }
    let issued = file
        .strip_prefix("bless-")?
        .strip_suffix(".json")?
        .parse()
        .ok()?;
    Timestamp::from_nanosecond(issued).ok()?;
    Some(commit)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;

    static_assertions::assert_impl_all!(ScopedBlessingRecord: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn scoped_record_and_key_round_trip() {
        let record = ScopedBlessingRecord {
            record: BlessingRecord::new(
                "commit".to_owned(),
                Timestamp::from_nanosecond(123_456_789).unwrap(),
                Vec::new(),
                "test".to_owned(),
            ),
            scope: BlessingScope::default(),
        };
        assert_eq!(
            ScopedBlessingRecord::from_json(&record.to_json().unwrap()).unwrap(),
            record
        );
        assert_eq!(
            parse_scoped_blessing_key(&record.key("project")),
            Some("commit")
        );
        assert!(record.key("a/b").starts_with("v1/a_b/objects/blessings/"));
        for key in [
            "v1/project/objects/blessings/commit/bless-invalid.json",
            "v1/project/objects/blessings/commit/clean.json",
            "v2/project/objects/blessings/commit/bless-1.json",
            "v1/project/other/blessings/commit/bless-1.json",
            "v1//objects/blessings/commit/bless-1.json",
            "v1/project/objects/blessings//bless-1.json",
            "v1/project/objects/callgrind/target/machine/commit/bless-1.json",
        ] {
            assert!(parse_scoped_blessing_key(key).is_none());
        }
        ScopedBlessingRecord::from_json(&record.record.to_json().unwrap()).unwrap_err();
    }
}
