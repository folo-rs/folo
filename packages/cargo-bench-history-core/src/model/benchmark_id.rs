//! The stable identity of a benchmark series and the prefix used to scope it.

use std::error::Error;
use std::fmt;
use std::str::FromStr;

use nonempty::NonEmpty;
use serde::{Deserialize, Serialize};

/// Stable identity of a benchmark series.
///
/// Two runs contribute to the same series if and only if their `BenchmarkId`
/// values are equal, so the identity must be reproducible across runs *and*
/// uniquely identify the benchmark within its project.
///
/// The identity is an ordered list of path-like segments, from coarsest to
/// finest. Each benchmark engine decides what its segments are — the general
/// model imposes no fixed `package`/`group`/`case` structure, because the
/// engines disagree on which of those they can even report:
///
/// * **Callgrind** (via Gungraun) emits `[package, module_path, function_name]`
///   plus an optional parameter segment.
/// * **Criterion** emits `[group_id, function_id?]` plus an optional parameter
///   segment; its machine-readable output records no owning package.
/// * **`alloc_tracker`** and **`all_the_time`** emit a single operation-name
///   segment.
///
/// Keeping the segments as a list (rather than fixed, partly optional fields)
/// lets each engine adapter own the mapping from its raw output to a comparable
/// identity, and lets prefix matching (used by `bless` and `analyze`) work
/// uniformly against the [`qualified`](Self::qualified) join of the segments.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct BenchmarkId {
    /// The identity segments, coarsest first, joined by `/` to form the qualified
    /// identity. A benchmark always has at least one segment, so the list is
    /// [`NonEmpty`].
    pub segments: NonEmpty<String>,
}

impl BenchmarkId {
    /// Creates a benchmark identity from its ordered segments.
    #[must_use]
    pub fn new(segments: NonEmpty<String>) -> Self {
        Self { segments }
    }

    /// The fully qualified identity: every segment joined by `/`. This form
    /// uniquely identifies the series and is what reports and prefix matching
    /// use, so cross-package collisions stay visible.
    #[must_use]
    pub fn qualified(&self) -> String {
        self.segments
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("/")
    }
}

impl fmt::Display for BenchmarkId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.qualified())
    }
}

/// A non-empty prefix of a benchmark's [qualified identity](BenchmarkId::qualified),
/// used to scope `bless` and `analyze` to a family of benchmarks.
///
/// Matching is a raw `starts_with` against the qualified identity, so `foo/bar`
/// accepts `foo/bar` and `foo/bar/baz`; append a trailing `/` to require a
/// whole-segment boundary. The empty string would accept every benchmark, which
/// is almost always a mistake, so the value is guaranteed non-empty — construct
/// one with [`new`](Self::new) (or `parse`/`TryFrom<String>`), each of which
/// rejects an empty input. The intent "accept every benchmark" is expressed by an
/// *empty list* of prefixes, never by an empty prefix.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(into = "String", try_from = "String")]
pub struct BenchmarkIdPrefix(String);

impl BenchmarkIdPrefix {
    /// Creates a prefix from a non-empty string.
    ///
    /// # Errors
    ///
    /// Returns [`EmptyBenchmarkIdPrefix`] if `value` is empty.
    pub fn new(value: impl Into<String>) -> Result<Self, EmptyBenchmarkIdPrefix> {
        let value = value.into();
        if value.is_empty() {
            return Err(EmptyBenchmarkIdPrefix);
        }
        Ok(Self(value))
    }

    /// The prefix as a string slice.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BenchmarkIdPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BenchmarkIdPrefix {
    type Err = EmptyBenchmarkIdPrefix;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl TryFrom<String> for BenchmarkIdPrefix {
    type Error = EmptyBenchmarkIdPrefix;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<BenchmarkIdPrefix> for String {
    fn from(prefix: BenchmarkIdPrefix) -> Self {
        prefix.0
    }
}

/// The error returned when constructing a [`BenchmarkIdPrefix`] from an empty
/// string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EmptyBenchmarkIdPrefix;

impl fmt::Display for EmptyBenchmarkIdPrefix {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a benchmark-id prefix must not be empty")
    }
}

impl Error for EmptyBenchmarkIdPrefix {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use nonempty::nonempty;

    use super::*;

    #[test]
    fn leading_segment_distinguishes_otherwise_equal_ids() {
        let foo = BenchmarkId::new(nonempty![
            "foo".to_owned(),
            "a::bench".to_owned(),
            "run".to_owned(),
        ]);
        let bar = BenchmarkId::new(nonempty![
            "bar".to_owned(),
            "a::bench".to_owned(),
            "run".to_owned(),
        ]);

        assert_ne!(foo, bar);
    }

    #[test]
    fn ids_sort_lexicographically_by_segments() {
        let bar = BenchmarkId::new(nonempty!["bar".to_owned(), "z".to_owned()]);
        let foo = BenchmarkId::new(nonempty!["foo".to_owned(), "a".to_owned()]);

        let mut ids = vec![foo.clone(), bar.clone()];
        ids.sort();

        assert_eq!(ids, vec![bar, foo]);
    }

    #[test]
    fn qualified_joins_segments() {
        let id = BenchmarkId::new(nonempty![
            "fast_time".to_owned(),
            "a::group".to_owned(),
            "capture".to_owned(),
            "two_instants".to_owned(),
        ]);
        assert_eq!(id.qualified(), "fast_time/a::group/capture/two_instants");
        assert_eq!(id.to_string(), id.qualified());
    }

    #[test]
    fn qualified_handles_a_single_segment() {
        let id = BenchmarkId::new(nonempty!["a::group".to_owned()]);
        assert_eq!(id.qualified(), "a::group");
    }

    #[test]
    fn benchmark_id_prefix_rejects_an_empty_string() {
        assert_eq!(BenchmarkIdPrefix::new(""), Err(EmptyBenchmarkIdPrefix));
        assert_eq!("".parse::<BenchmarkIdPrefix>(), Err(EmptyBenchmarkIdPrefix));
        assert_eq!(
            BenchmarkIdPrefix::try_from(String::new()),
            Err(EmptyBenchmarkIdPrefix)
        );
    }

    #[test]
    fn benchmark_id_prefix_keeps_a_non_empty_value() {
        let prefix = BenchmarkIdPrefix::new("foo/bar").unwrap();
        assert_eq!(prefix.as_str(), "foo/bar");
        assert_eq!(prefix.to_string(), "foo/bar");
    }

    #[test]
    fn benchmark_id_prefix_serializes_as_a_plain_string() {
        let prefix = BenchmarkIdPrefix::new("foo/bar").unwrap();
        let json = serde_json::to_string(&prefix).unwrap();
        assert_eq!(json, "\"foo/bar\"");
        let restored: BenchmarkIdPrefix = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, prefix);
    }

    #[test]
    fn benchmark_id_prefix_deserialization_rejects_an_empty_string() {
        let error = serde_json::from_str::<BenchmarkIdPrefix>("\"\"").unwrap_err();
        assert!(error.to_string().contains("benchmark-id prefix"), "{error}");
    }
}
