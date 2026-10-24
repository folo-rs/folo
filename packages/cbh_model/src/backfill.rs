//! Collection-only completion evidence shared by the core and workflow companion.

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

/// A completed backfill pass, not an analysis verdict or an execution-success flag.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "CompletionWire")]
pub struct BackfillCompletion {
    version: u32,
    from: String,
    to: String,
    total: usize,
    counts: BackfillCounts,
    stop_reason: BackfillStop,
    overwrite: bool,
}

impl BackfillCompletion {
    /// Binds final replay counts to the resolved inclusive range.
    ///
    /// # Errors
    ///
    /// Returns an error for inconsistent counts, endpoints or stopping conditions.
    pub fn new(
        from: String,
        to: String,
        total: usize,
        counts: BackfillCounts,
        stop_reason: BackfillStop,
        overwrite: bool,
    ) -> Result<Self, serde_json::Error> {
        Self::try_from(CompletionWire {
            version: COMPLETION_VERSION,
            from,
            to,
            total,
            counts,
            stop_reason,
            overwrite,
        })
    }

    /// Reads validated completion evidence.
    ///
    /// # Errors
    ///
    /// Returns an error for unsupported or inconsistent completion evidence.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }

    /// Encodes completion evidence independently of log formatting.
    ///
    /// # Errors
    ///
    /// Returns an error if JSON serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// The oldest resolved commit in the inclusive range.
    #[must_use]
    pub fn from(&self) -> &str {
        &self.from
    }

    /// The newest resolved commit in the inclusive range.
    #[must_use]
    pub fn to(&self) -> &str {
        &self.to
    }

    /// Final per-commit counts across the entire range.
    #[must_use]
    pub fn counts(&self) -> &BackfillCounts {
        &self.counts
    }

    /// The reason replay stopped, independently of ignored per-commit failures.
    #[must_use]
    pub fn stop_reason(&self) -> BackfillStop {
        self.stop_reason
    }

    /// Whether this pass replaced results instead of skipping recorded commits.
    #[must_use]
    pub fn overwrite(&self) -> bool {
        self.overwrite
    }
}

impl TryFrom<CompletionWire> for BackfillCompletion {
    type Error = serde_json::Error;

    fn try_from(raw: CompletionWire) -> Result<Self, Self::Error> {
        let counts = &raw.counts;
        let total = [
            counts.stored,
            counts.skipped,
            counts.empty,
            counts.failed,
            counts.deferred,
        ]
        .into_iter()
        .try_fold(0_usize, usize::checked_add);
        let stop_matches = match raw.stop_reason {
            BackfillStop::RangeExhausted => counts.deferred == 0,
            BackfillStop::CommitLimit => counts.deferred > 0,
            BackfillStop::BenchmarkFailure => counts.failed > 0,
        };
        if raw.version != COMPLETION_VERSION
            || !full_commit(&raw.from)
            || !full_commit(&raw.to)
            || raw.total == 0
            || total != Some(raw.total)
            || !stop_matches
        {
            return Err(Self::Error::custom("inconsistent backfill completion"));
        }
        Ok(Self {
            version: raw.version,
            from: raw.from,
            to: raw.to,
            total: raw.total,
            counts: raw.counts,
            stop_reason: raw.stop_reason,
            overwrite: raw.overwrite,
        })
    }
}

/// Mutually exclusive commit classifications, including work left unattempted.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BackfillCounts {
    /// Commits that stored measurements.
    pub stored: usize,
    /// Commits with existing results, including write-time duplicates.
    pub skipped: usize,
    /// Completed attempts that harvested no compatible benchmark cases.
    pub empty: usize,
    /// Per-commit build or benchmark failures, not infrastructure failures.
    pub failed: usize,
    /// Eligible commits left unattempted.
    pub deferred: usize,
}

/// Normal replay limits and benchmark failures remain distinct.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BackfillStop {
    /// Every eligible commit was attempted.
    RangeExhausted,
    /// The configured attempt budget left eligible commits unattempted.
    CommitLimit,
    /// A per-commit failure stopped replay.
    BenchmarkFailure,
}

// This handoff is independent of measurement-storage and analysis-report versions.
const COMPLETION_VERSION: u32 = 1;

/// Unchecked transport data never reaches workflow presentation directly.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompletionWire {
    version: u32,
    from: String,
    to: String,
    total: usize,
    counts: BackfillCounts,
    stop_reason: BackfillStop,
    overwrite: bool,
}

/// Endpoints use the same full Git SHA-1 identities as collection snapshots.
fn full_commit(value: &str) -> bool {
    const COMMIT_HEX_LENGTH: usize = 40;
    value.len() == COMMIT_HEX_LENGTH && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(BackfillCompletion: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn completion_roundtrips_each_stop_reason() {
        for (stop, deferred) in [
            (BackfillStop::RangeExhausted, 0),
            (BackfillStop::CommitLimit, 1),
            (BackfillStop::BenchmarkFailure, 1),
        ] {
            let completion = BackfillCompletion::new(
                "a".repeat(40),
                "b".repeat(40),
                4 + deferred,
                BackfillCounts {
                    stored: 1,
                    skipped: 1,
                    empty: 1,
                    failed: 1,
                    deferred,
                },
                stop,
                true,
            )
            .unwrap();
            assert_eq!(
                BackfillCompletion::from_slice(completion.to_json().unwrap().as_bytes()).unwrap(),
                completion
            );
            assert_eq!(completion.from(), "a".repeat(40));
            assert_eq!(completion.to(), "b".repeat(40));
            assert_eq!(completion.counts().deferred, deferred);
            assert_eq!(completion.stop_reason(), stop);
            assert!(completion.overwrite());
        }
    }

    #[test]
    fn completion_roundtrips_each_write_mode() {
        for overwrite in [false, true] {
            let completion = BackfillCompletion::new(
                "a".repeat(40),
                "b".repeat(40),
                1,
                BackfillCounts {
                    stored: 1,
                    ..BackfillCounts::default()
                },
                BackfillStop::RangeExhausted,
                overwrite,
            )
            .unwrap();
            assert_eq!(completion.overwrite(), overwrite);

            let json = completion.to_json().unwrap();
            let wire: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(wire.get("overwrite").unwrap(), overwrite);

            let decoded = BackfillCompletion::from_slice(json.as_bytes()).unwrap();
            assert_eq!(decoded.overwrite(), overwrite);
            assert_eq!(decoded, completion);
        }
    }

    #[test]
    fn inconsistent_counts_and_stops_are_rejected() {
        for (total, counts, stop) in [
            (0, BackfillCounts::default(), BackfillStop::RangeExhausted),
            (1, BackfillCounts::default(), BackfillStop::RangeExhausted),
            (
                1,
                BackfillCounts {
                    stored: 1,
                    ..BackfillCounts::default()
                },
                BackfillStop::CommitLimit,
            ),
            (
                1,
                BackfillCounts {
                    deferred: 1,
                    ..BackfillCounts::default()
                },
                BackfillStop::RangeExhausted,
            ),
            (
                1,
                BackfillCounts {
                    stored: 1,
                    ..BackfillCounts::default()
                },
                BackfillStop::BenchmarkFailure,
            ),
            (
                1,
                BackfillCounts {
                    stored: usize::MAX,
                    skipped: 2,
                    ..BackfillCounts::default()
                },
                BackfillStop::RangeExhausted,
            ),
        ] {
            BackfillCompletion::new("a".repeat(40), "b".repeat(40), total, counts, stop, false)
                .unwrap_err();
        }
    }

    #[test]
    fn malformed_identity_and_unknown_schema_are_rejected() {
        let completion = BackfillCompletion::new(
            "a".repeat(40),
            "b".repeat(40),
            1,
            BackfillCounts {
                skipped: 1,
                ..BackfillCounts::default()
            },
            BackfillStop::RangeExhausted,
            false,
        )
        .unwrap();
        let raw: serde_json::Value = serde_json::from_str(&completion.to_json().unwrap()).unwrap();
        for (key, value) in [
            ("version", serde_json::json!(2)),
            ("from", serde_json::json!("HEAD")),
            ("to", serde_json::json!("g".repeat(40))),
            ("extra", serde_json::json!(true)),
        ] {
            let mut raw = raw.clone();
            raw.as_object_mut().unwrap().insert(key.to_owned(), value);
            BackfillCompletion::from_slice(raw.to_string().as_bytes()).unwrap_err();
        }
    }
}
