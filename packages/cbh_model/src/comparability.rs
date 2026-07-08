//! Comparability: deciding which runs may be compared to each other, and how the
//! storage is partitioned so that only comparable runs share a series.
//!
//! The guiding rule (see the *Comparability & storage partitioning* section of
//! `DESIGN.md`) is to partition only by what makes results *fundamentally*
//! incomparable — project, engine, target triple, and (for hardware-dependent
//! engines) a machine key — and to record everything else as metadata so its
//! effect stays visible in the timeline.

use std::fmt;

use serde::Serialize;

use super::constants::{OBJECTS_SEGMENT, STORAGE_VERSION};

/// A benchmark engine, distinguished by whether its results depend on hardware.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Engine {
    /// Criterion wall-clock benchmarks: hardware-dependent and noisy.
    Criterion,
    /// Callgrind (via Gungraun) instruction counts: simulated, hardware-independent.
    Callgrind,
    /// `alloc_tracker` allocation counts and bytes: hardware-independent but not
    /// deterministic — warmup and buffer-resize allocations jitter the per-iteration
    /// figure, which is amortized over a Criterion-chosen iteration count.
    AllocTracker,
    /// `all_the_time` processor-time measurements: hardware-dependent and noisy.
    AllTheTime,
}

impl Engine {
    /// Every supported engine, in a stable order used to inject the combined
    /// benchmark environment and to harvest each engine's output tree after the
    /// single `cargo bench` invocation.
    pub const ALL: [Self; 4] = [
        Self::Callgrind,
        Self::Criterion,
        Self::AllocTracker,
        Self::AllTheTime,
    ];

    /// The stable lowercase identifier used in storage paths and config keys.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Criterion => "criterion",
            Self::Callgrind => "callgrind",
            Self::AllocTracker => "alloc_tracker",
            Self::AllTheTime => "all_the_time",
        }
    }

    /// Parses an [`Engine`] from its stable lowercase identifier.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "criterion" => Some(Self::Criterion),
            "callgrind" => Some(Self::Callgrind),
            "alloc_tracker" => Some(Self::AllocTracker),
            "all_the_time" => Some(Self::AllTheTime),
            _ => None,
        }
    }

    /// Whether results from this engine depend on the host hardware.
    ///
    /// Hardware-dependent engines require a machine key in their partition;
    /// hardware-independent ones use the literal `synthetic` instead. Allocation
    /// counts and bytes are a property of the code, not the machine, so
    /// `alloc_tracker` is hardware-independent; processor time obviously is not.
    #[must_use]
    pub fn is_hardware_dependent(self) -> bool {
        match self {
            Self::Criterion | Self::AllTheTime => true,
            Self::Callgrind | Self::AllocTracker => false,
        }
    }
}

impl fmt::Display for Engine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The set of factors that must match for two runs in a project to share a series.
///
/// A discriminant set is `engine / target_triple / machine`. Within a single
/// project all runs that share a discriminant set are comparable; runs in
/// different sets (a different engine, target triple, or machine key) never share
/// a series. It is both the value `run` writes under and the value `analyze` reads
/// back (parsed from a storage key), so the same type drives both sides.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct DiscriminantSet {
    /// Engine identifier (for example, `callgrind`).
    pub engine: String,
    /// Resolved target triple the run was recorded under.
    pub target_triple: String,
    /// Machine key (`synthetic` for hardware-independent engines).
    pub machine_key: String,
}

impl DiscriminantSet {
    /// Creates a discriminant set, sanitizing the path-forming components.
    ///
    /// `target_triple` and `machine_key` are sanitized so that every segment is a
    /// single, well-formed path component: any character that is not ASCII
    /// alphanumeric, `-`, `_`, or `.` is replaced with `_`, and a segment that
    /// would otherwise be empty or consist only of dots becomes `_`. This keeps a
    /// stray `/` (or other surprising input) from silently splitting a storage key
    /// into the wrong number of segments. A `None` machine key becomes the literal
    /// `synthetic`, used for hardware-independent engines.
    #[must_use]
    pub fn new(engine: Engine, target_triple: &str, machine_key: Option<&str>) -> Self {
        Self {
            engine: engine.as_str().to_owned(),
            target_triple: sanitize_segment(target_triple),
            machine_key: machine_key.map_or_else(|| "synthetic".to_owned(), sanitize_segment),
        }
    }

    /// Whether this is a hardware-independent (`synthetic`) set.
    #[must_use]
    pub fn is_synthetic(&self) -> bool {
        self.machine_key == "synthetic"
    }

    /// The storage prefix that all runs in this series share, within `project`.
    ///
    /// Layout:
    /// `{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/{engine}/{target_triple}/{machine|synthetic}`.
    /// The fixed `objects` segment separates the data subtree from a project's
    /// metadata siblings (e.g. the cache-invalidation marker); below this prefix the
    /// history is organized by commit (see [`clean_key`] and [`dirty_key`]) so
    /// `analyze` can resolve a series from git topology.
    ///
    /// [`clean_key`]: Self::clean_key
    /// [`dirty_key`]: Self::dirty_key
    #[must_use]
    pub fn partition_prefix(&self, project: &str) -> String {
        let project = sanitize_segment(project);
        let engine = &self.engine;
        let triple = &self.target_triple;
        let machine_key = &self.machine_key;
        format!("{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/{engine}/{triple}/{machine_key}")
    }

    /// The object key for the canonical (clean working tree) result at `commit`.
    ///
    /// Layout: `{prefix}/{commit}/clean.json`. A clean run is keyed solely by its
    /// commit, so it is deterministic: a second clean run of the same commit maps
    /// to the same key and collides, which the write-once storage detects so `run`
    /// can refuse the duplicate unless an overwrite is explicitly requested.
    ///
    /// `commit` is sanitized so the directory name always forms a single segment.
    #[must_use]
    pub fn clean_key(&self, project: &str, commit: &str) -> String {
        let prefix = self.partition_prefix(project);
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/clean.json")
    }

    /// The object key for a dirty (uncommitted-changes) snapshot at `commit`,
    /// observed at `observation_unix`.
    ///
    /// Layout: `{prefix}/{commit}/dirty-{observation_unix}.json`. Because a dirty
    /// snapshot does not correspond to committed code, it is distinguished by its
    /// observation time rather than by the commit alone, so multiple dirty
    /// snapshots on the same base commit coexist; only two snapshots sharing an
    /// observation second collide.
    ///
    /// `commit` is sanitized so the directory name always forms a single segment.
    #[must_use]
    pub fn dirty_key(&self, project: &str, commit: &str, observation_unix: i64) -> String {
        let prefix = self.partition_prefix(project);
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/dirty-{observation_unix}.json")
    }

    /// The blessing sidecar key for this set's commit directory, issued at
    /// `issued_unix`.
    ///
    /// Layout: `{prefix}/{commit}/bless-{issued_unix}.json`. `commit` is sanitized
    /// so the directory name always forms a single segment.
    #[must_use]
    pub fn bless_key(&self, project: &str, commit: &str, issued_unix: i64) -> String {
        let prefix = self.partition_prefix(project);
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/bless-{issued_unix}.json")
    }

    /// The storage prefix shared by every object recorded at `commit` in this
    /// partition (`{prefix}/{commit}/`), used to enumerate a commit directory.
    ///
    /// `commit` is sanitized so the directory name always forms a single segment.
    #[must_use]
    pub fn commit_prefix(&self, project: &str, commit: &str) -> String {
        let prefix = self.partition_prefix(project);
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/")
    }
}

impl fmt::Display for DiscriminantSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{}/{}",
            self.engine, self.target_triple, self.machine_key
        )
    }
}

/// Replaces every character that is not safe in a single path segment with `_`,
/// mapping an otherwise-empty or all-dots result to `_`.
///
/// "Safe" is the conservative set `[A-Za-z0-9._-]`, which is valid both as a
/// filesystem path component (for local storage) and as an Azure blob name part.
/// Mangling rather than rejecting means the tool never refuses a run merely
/// because its project, triple, or machine key contains an awkward character.
#[must_use]
pub fn sanitize_segment(raw: &str) -> String {
    let mangled: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if mangled.is_empty() || mangled.chars().all(|c| c == '.') {
        return "_".to_owned();
    }
    mangled
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn hardware_dependence_matches_engine() {
        assert!(Engine::Criterion.is_hardware_dependent());
        assert!(Engine::AllTheTime.is_hardware_dependent());
        assert!(!Engine::Callgrind.is_hardware_dependent());
        assert!(!Engine::AllocTracker.is_hardware_dependent());
    }

    #[test]
    fn alloc_tracker_uses_a_synthetic_partition() {
        // Allocation counts are a property of the code, not the machine, so
        // `alloc_tracker` carries no machine key.
        let set = DiscriminantSet::new(Engine::AllocTracker, "x86_64-pc-windows-msvc", None);
        assert!(set.is_synthetic());
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/alloc_tracker/x86_64-pc-windows-msvc/synthetic"
        );
    }

    #[test]
    fn all_the_time_partitions_by_machine_key() {
        // Processor time depends on the machine, so `all_the_time` carries a
        // machine fingerprint.
        let set =
            DiscriminantSet::new(Engine::AllTheTime, "x86_64-pc-windows-msvc", Some("abc123"));
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/all_the_time/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn machine_key_appears_in_partition() {
        let set = DiscriminantSet::new(Engine::Criterion, "x86_64-pc-windows-msvc", Some("abc123"));
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/criterion/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn clean_key_is_named_by_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        assert_eq!(
            set.clean_key("folo", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json"
        );
    }

    #[test]
    fn dirty_key_is_named_by_commit_and_observation_time() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        assert_eq!(
            set.dirty_key(
                "folo",
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                1_700_000_000
            ),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/dirty-1700000000.json"
        );
    }

    #[test]
    fn bless_key_targets_the_commit_directory() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        assert_eq!(
            set.bless_key("folo", "abc123", 1_700_000_000),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/bless-1700000000.json"
        );
    }

    #[test]
    fn commit_prefix_enumerates_one_commit_directory() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        assert_eq!(
            set.commit_prefix("folo", "dead/beef"),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/"
        );
    }

    #[test]
    fn engine_display_matches_as_str() {
        assert_eq!(Engine::Criterion.to_string(), "criterion");
        assert_eq!(Engine::Callgrind.to_string(), "callgrind");
        assert_eq!(Engine::AllocTracker.to_string(), "alloc_tracker");
        assert_eq!(Engine::AllTheTime.to_string(), "all_the_time");
    }

    #[test]
    fn engine_from_name_roundtrips() {
        for engine in Engine::ALL {
            assert_eq!(Engine::from_name(engine.as_str()), Some(engine));
        }
        assert_eq!(Engine::from_name("dhat"), None);
    }

    #[test]
    fn sanitize_segment_keeps_safe_characters() {
        assert_eq!(
            sanitize_segment("x86_64-unknown-linux-gnu"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(sanitize_segment("my.project-1"), "my.project-1");
    }

    #[test]
    fn sanitize_segment_replaces_separators_and_specials() {
        assert_eq!(sanitize_segment("team/app"), "team_app");
        assert_eq!(sanitize_segment(r"team\app"), "team_app");
        assert_eq!(sanitize_segment("weird:name"), "weird_name");
        assert_eq!(sanitize_segment("with space"), "with_space");
        assert_eq!(sanitize_segment("café"), "caf_");
    }

    #[test]
    fn sanitize_segment_maps_empty_and_dot_only_to_underscore() {
        assert_eq!(sanitize_segment(""), "_");
        assert_eq!(sanitize_segment("."), "_");
        assert_eq!(sanitize_segment(".."), "_");
    }

    #[test]
    fn new_sanitizes_partition_components() {
        let set = DiscriminantSet::new(Engine::Criterion, "weird/triple", Some("machine/one"));
        assert_eq!(
            set.partition_prefix("team/app"),
            "v1/team_app/objects/criterion/weird_triple/machine_one"
        );
        // The partition prefix has exactly the six canonical segments.
        assert_eq!(set.partition_prefix("team/app").split('/').count(), 6);
    }

    #[test]
    fn clean_key_sanitizes_the_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        let object = set.clean_key("folo", "dead/beef");
        assert_eq!(
            object,
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/clean.json"
        );
        // Exactly the eight canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 8);
    }

    #[test]
    fn dirty_key_sanitizes_the_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", None);
        let object = set.dirty_key("folo", "dead/beef", 1_700_000_000);
        assert_eq!(
            object,
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/dirty-1700000000.json"
        );
        // Exactly the eight canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 8);
    }
}
