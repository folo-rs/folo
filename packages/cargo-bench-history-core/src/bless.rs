//! The blessing data model: a manual acceptance of a benchmark's current level on
//! the base branch, so history analysis stops re-flagging an intentional change.
//!
//! A blessing is an append-only sidecar (`bless-<issued_unix>.json`, described by
//! the `bless` / `unbless` command in `DESIGN.md`) stored in the same commit
//! directory as the run it accepts. It names one
//! or more benchmark-id prefixes; a series whose qualified id starts with any of
//! them is re-baselined to the blessed commit, so the accepted step stops being
//! reported as a regression while its pre-blessing history is still retained for
//! charts and longer-range analysis. Sidecars are never mutated: multiple
//! blessings on one commit coexist and are unioned at query time, and editing a
//! blessing means `unbless`-ing and re-blessing.

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::model::BenchmarkId;

/// Schema version of the stored [`BlessingRecord`] JSON.
///
/// Bumped whenever the on-disk representation changes in a backward-incompatible
/// way so that `analyze` can refuse or migrate older data.
pub const BLESS_SCHEMA_VERSION: u32 = 1;

/// A single blessing: which benchmarks were accepted, at which commit, and when.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BlessingRecord {
    /// Schema version of this record (see [`BLESS_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Full commit SHA the blessing was issued at (the blessed data point).
    pub commit: String,
    /// Committer date of the blessed commit, used to label and anchor the
    /// blessing in reports and charts.
    pub commit_time: Timestamp,
    /// Wall-clock time at which the blessing was issued (provenance).
    pub issued_at: Timestamp,
    /// Benchmark-id prefixes this blessing accepts, matched against
    /// [`BenchmarkId::qualified`]. A prefix is a raw `starts_with` test, so
    /// `foo/bar` accepts `foo/bar` and `foo/bar/baz`; append a trailing `/` to
    /// require a whole-segment boundary.
    pub prefixes: Vec<String>,
    /// Version of the tool that issued the blessing.
    pub tool_version: String,
}

impl BlessingRecord {
    /// Creates a blessing record stamped with the current schema version.
    #[must_use]
    pub fn new(
        commit: String,
        commit_time: Timestamp,
        issued_at: Timestamp,
        prefixes: Vec<String>,
        tool_version: String,
    ) -> Self {
        Self {
            schema_version: BLESS_SCHEMA_VERSION,
            commit,
            commit_time,
            issued_at,
            prefixes,
            tool_version,
        }
    }

    /// Serializes this blessing to pretty-printed JSON, the on-disk format.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization fails.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    /// Deserializes a blessing from its JSON representation.
    ///
    /// # Errors
    ///
    /// Returns an error if `json` is not a valid serialized blessing.
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Whether any of this blessing's prefixes accepts `id`.
    ///
    /// The match is a raw `starts_with` against the benchmark's qualified
    /// identity, so a prefix may select a whole family of benchmarks at once.
    #[must_use]
    pub fn matches(&self, id: &BenchmarkId) -> bool {
        let qualified = id.qualified();
        self.prefixes
            .iter()
            .any(|prefix| qualified.starts_with(prefix.as_str()))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    fn id(
        package: Option<&str>,
        group: &str,
        case: Option<&str>,
        value: Option<&str>,
    ) -> BenchmarkId {
        let segments = [package, Some(group), case, value]
            .into_iter()
            .flatten()
            .map(ToOwned::to_owned)
            .collect();
        BenchmarkId::new(segments)
    }

    fn record(prefixes: &[&str]) -> BlessingRecord {
        BlessingRecord::new(
            "deadbeef".to_owned(),
            ts(1_700_000_000),
            ts(1_700_000_100),
            prefixes.iter().map(|prefix| (*prefix).to_owned()).collect(),
            "0.0.1".to_owned(),
        )
    }

    #[test]
    fn json_round_trips() {
        let original = BlessingRecord::new(
            "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_owned(),
            ts(1_700_000_000),
            ts(1_700_000_100),
            vec!["all_the_time/read_cell".to_owned()],
            "1.2.3".to_owned(),
        );
        let json = original.to_json().unwrap();
        let parsed = BlessingRecord::from_json(&json).unwrap();
        assert_eq!(parsed, original);
    }

    #[test]
    fn prefix_matches_exact_and_family() {
        let blessing = record(&["all_the_time/read_cell"]);
        // Exact qualified id.
        assert!(blessing.matches(&id(Some("all_the_time"), "read_cell", None, None)));
        // A deeper id under the same prefix.
        assert!(blessing.matches(&id(Some("all_the_time"), "read_cell", Some("warm"), None)));
        // A sibling that merely shares a leading directory does not match.
        assert!(!blessing.matches(&id(Some("all_the_time"), "write_cell", None, None)));
    }

    #[test]
    fn partial_segment_prefix_matches_a_family() {
        // A deliberate partial-segment prefix accepts every id whose qualified
        // form starts with it, crossing a segment boundary.
        let blessing = record(&["overhead/groups_"]);
        assert!(blessing.matches(&id(None, "overhead", Some("groups_10"), None)));
        assert!(blessing.matches(&id(None, "overhead", Some("groups_100"), None)));
        assert!(!blessing.matches(&id(None, "overhead", Some("single"), None)));
    }

    #[test]
    fn any_matching_prefix_accepts() {
        let blessing = record(&["foo/bar", "baz/qux"]);
        assert!(blessing.matches(&id(None, "baz", Some("qux"), None)));
    }
}
