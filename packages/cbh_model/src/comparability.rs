//! Comparability: deciding which runs may be compared to each other, and how the
//! storage is partitioned so that only comparable runs share a series.
//!
//! The guiding rule (see the *Comparability & storage partitioning* section of
//! `DESIGN.md`) is to partition only by what makes results *fundamentally*
//! incomparable — project, engine, target triple, and a machine key — and to
//! record everything else as metadata so its effect stays visible in the timeline.

use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};

use super::constants::{OBJECTS_SEGMENT, STORAGE_VERSION};

/// A benchmark engine: the measurement tool whose output a series accumulates.
///
/// Serializes to (and deserializes from) its stable lowercase
/// [identifier](Self::as_str) — `criterion`, `callgrind`, `alloc_tracker`,
/// `all_the_time` — so a [`DiscriminantSet`] flattened into a JSON report keeps
/// that exact wire name.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Engine {
    /// Criterion wall-clock benchmarks: noisy, with a confidence interval.
    Criterion,
    /// Callgrind (via Gungraun) instruction counts: simulated and low-noise, yet
    /// still machine-dependent (microarchitecture-specific library dispatch moves
    /// the counts), so its history is partitioned by machine key like every engine.
    Callgrind,
    /// `alloc_tracker` allocation counts and bytes: not deterministic — warmup and
    /// buffer-resize allocations jitter the per-iteration figure, which is amortized
    /// over a Criterion-chosen iteration count.
    AllocTracker,
    /// `all_the_time` processor-time measurements: noisy, with a confidence interval.
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
}

/// Orders engines by their stable [identifier](Self::as_str), so a
/// [`DiscriminantSet`] sorts identically whether its engine is compared as an
/// `Engine` or as the serialized string that used to stand in for it.
impl Ord for Engine {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl PartialOrd for Engine {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
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
    /// The benchmark engine whose output the series accumulates.
    pub engine: Engine,
    /// Resolved target triple the run was recorded under.
    pub target_triple: String,
    /// Machine key: a stable hardware fingerprint (or an explicit override). Every
    /// engine is machine-keyed.
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
    /// into the wrong number of segments.
    #[must_use]
    pub fn new(engine: Engine, target_triple: &str, machine_key: &str) -> Self {
        Self {
            engine,
            target_triple: sanitize_segment(target_triple),
            machine_key: sanitize_segment(machine_key),
        }
    }

    /// The storage prefix that all runs in this series share, within `project`.
    ///
    /// Layout:
    /// `{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/{engine}/{target_triple}/{machine}`.
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
        let engine = self.engine.as_str();
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

/// Which kind of object a storage key names, decoded from its file segment.
///
/// A commit directory holds three kinds of object, distinguished by file name:
/// the canonical clean run (`clean.json`), zero or more dirty snapshots
/// (`dirty-<observation_unix>.json`), and zero or more blessing sidecars
/// (`bless-<issued_unix>.json`). Recovering this typed classification once
/// (rather than re-testing string prefixes at each use site) keeps the three
/// cases exhaustive and parses each embedded timestamp a single time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectKind {
    /// The canonical clean-working-tree run (`clean.json`).
    Clean,
    /// A dirty (uncommitted-tree) snapshot, distinguished by its observation second.
    Dirty {
        /// The Unix second at which the snapshot was observed, from the file name.
        observation_unix: i64,
    },
    /// A blessing sidecar, distinguished by the second it was issued.
    Bless {
        /// The Unix second at which the blessing was issued, from the file name.
        issued_unix: i64,
    },
}

impl ObjectKind {
    /// Decodes the file segment of a storage key into its object kind, returning
    /// `None` when the segment is not one of the three recognized object files.
    fn from_file_segment(file: &str) -> Option<Self> {
        if file == "clean.json" {
            return Some(Self::Clean);
        }
        if let Some(seconds) = file
            .strip_prefix("dirty-")
            .and_then(|rest| rest.strip_suffix(".json"))
        {
            return seconds
                .parse()
                .ok()
                .map(|observation_unix| Self::Dirty { observation_unix });
        }
        if let Some(seconds) = file
            .strip_prefix("bless-")
            .and_then(|rest| rest.strip_suffix(".json"))
        {
            return seconds
                .parse()
                .ok()
                .map(|issued_unix| Self::Bless { issued_unix });
        }
        None
    }
}

/// The components a storage key decomposes into.
///
/// A storage key references one of three kinds of object in a commit directory:
/// a clean run (`clean.json`), a dirty snapshot (`dirty-<unix>.json`), or a
/// blessing sidecar (`bless-<unix>.json`). The [`kind`](Self::kind) field, an
/// [`ObjectKind`], captures which; [`is_clean`](Self::is_clean),
/// [`is_dirty`](Self::is_dirty), and [`is_bless`](Self::is_bless) classify it.
///
/// This is the inverse of the key-construction methods above ([`clean_key`],
/// [`dirty_key`], [`bless_key`]): [`parse_key`] recovers this decomposition from a
/// stored object's key so `analyze` can group objects into comparable series.
///
/// [`clean_key`]: DiscriminantSet::clean_key
/// [`dirty_key`]: DiscriminantSet::dirty_key
/// [`bless_key`]: DiscriminantSet::bless_key
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageKey {
    /// The (sanitized) project segment.
    pub project: String,
    /// The discriminant set the key belongs to.
    pub set: DiscriminantSet,
    /// The commit directory segment (full commit ID, or `unknown`).
    pub commit: String,
    /// Which kind of object the key names, decoded from its file segment.
    pub kind: ObjectKind,
}

impl StorageKey {
    /// Whether the key names a clean (committed-tree) run.
    #[must_use]
    pub fn is_clean(&self) -> bool {
        matches!(self.kind, ObjectKind::Clean)
    }

    /// Whether the key names a dirty (uncommitted-tree) snapshot.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        matches!(self.kind, ObjectKind::Dirty { .. })
    }

    /// Whether the key names a blessing sidecar rather than a stored run.
    #[must_use]
    pub fn is_bless(&self) -> bool {
        matches!(self.kind, ObjectKind::Bless { .. })
    }

    /// The blessing sidecar key for this set's commit directory, issued at
    /// `issued_unix`.
    #[must_use]
    pub fn bless_key(&self, issued_unix: i64) -> String {
        self.set.bless_key(&self.project, &self.commit, issued_unix)
    }
}

/// Parses a storage object key into its components.
///
/// Keys have the form
/// `{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/{engine}/{triple}/{machine_key}/{commit}/{file}`
/// — exactly eight non-empty segments, with the fixed `objects` segment directly
/// under the project. Any key that does not match that shape exactly (wrong
/// version, missing `objects` segment, too few or too many segments, or an empty
/// segment), names an engine the tool has no adapter for, or whose file segment is
/// not one of the three recognized object files, is ignored (returns `None`)
/// rather than misattributed — so a per-project metadata sibling such as the
/// cache-invalidation marker is skipped.
#[must_use]
pub fn parse_key(key: &str) -> Option<StorageKey> {
    let parts: Vec<&str> = key.split('/').collect();
    let [
        version,
        project,
        objects,
        engine,
        target_triple,
        machine_key,
        commit,
        file,
    ] = parts.as_slice()
    else {
        return None;
    };
    if *version != STORAGE_VERSION || *objects != OBJECTS_SEGMENT {
        return None;
    }
    if parts.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let engine = Engine::from_name(engine)?;
    let kind = ObjectKind::from_file_segment(file)?;
    Some(StorageKey {
        project: (*project).to_owned(),
        set: DiscriminantSet {
            engine,
            target_triple: (*target_triple).to_owned(),
            machine_key: (*machine_key).to_owned(),
        },
        commit: (*commit).to_owned(),
        kind,
    })
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
    fn every_engine_partitions_by_machine_key() {
        // Every engine is machine-keyed: even the low-noise simulated counts differ
        // by microarchitecture, so a machine key always appears in the partition.
        let set = DiscriminantSet::new(Engine::AllocTracker, "x86_64-pc-windows-msvc", "abc123");
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/alloc_tracker/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn all_the_time_partitions_by_machine_key() {
        // Processor time depends on the machine, so `all_the_time` carries a
        // machine fingerprint.
        let set = DiscriminantSet::new(Engine::AllTheTime, "x86_64-pc-windows-msvc", "abc123");
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/all_the_time/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn machine_key_appears_in_partition() {
        let set = DiscriminantSet::new(Engine::Criterion, "x86_64-pc-windows-msvc", "abc123");
        assert_eq!(
            set.partition_prefix("folo"),
            "v1/folo/objects/criterion/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn display_formats_engine_triple_and_machine_key() {
        let set = DiscriminantSet::new(Engine::Criterion, "x86_64-pc-windows-msvc", "abc123");
        assert_eq!(set.to_string(), "criterion/x86_64-pc-windows-msvc/abc123");
    }

    #[test]
    fn clean_key_is_named_by_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "abc123");
        assert_eq!(
            set.clean_key("folo", "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/abc123/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json"
        );
    }

    #[test]
    fn dirty_key_is_named_by_commit_and_observation_time() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "abc123");
        assert_eq!(
            set.dirty_key(
                "folo",
                "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef",
                1_700_000_000
            ),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/abc123/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/dirty-1700000000.json"
        );
    }

    #[test]
    fn bless_key_targets_the_commit_directory() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "m1");
        assert_eq!(
            set.bless_key("folo", "abc123", 1_700_000_000),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/abc123/bless-1700000000.json"
        );
    }

    #[test]
    fn commit_prefix_enumerates_one_commit_directory() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "m1");
        assert_eq!(
            set.commit_prefix("folo", "dead/beef"),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/dead_beef/"
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
        let set = DiscriminantSet::new(Engine::Criterion, "weird/triple", "machine/one");
        assert_eq!(
            set.partition_prefix("team/app"),
            "v1/team_app/objects/criterion/weird_triple/machine_one"
        );
        // The partition prefix has exactly the six canonical segments.
        assert_eq!(set.partition_prefix("team/app").split('/').count(), 6);
    }

    #[test]
    fn clean_key_sanitizes_the_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "m1");
        let object = set.clean_key("folo", "dead/beef");
        assert_eq!(
            object,
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/dead_beef/clean.json"
        );
        // Exactly the eight canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 8);
    }

    #[test]
    fn dirty_key_sanitizes_the_commit() {
        let set = DiscriminantSet::new(Engine::Callgrind, "x86_64-unknown-linux-gnu", "m1");
        let object = set.dirty_key("folo", "dead/beef", 1_700_000_000);
        assert_eq!(
            object,
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/dead_beef/dirty-1700000000.json"
        );
        // Exactly the eight canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 8);
    }

    #[test]
    fn parse_key_decomposes_a_clean_key() {
        let parsed =
            parse_key("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/abc123/clean.json")
                .unwrap();
        assert_eq!(parsed.project, "folo");
        assert_eq!(parsed.set.engine, Engine::Callgrind);
        assert_eq!(parsed.set.target_triple, "x86_64-unknown-linux-gnu");
        assert_eq!(parsed.set.machine_key, "m1");
        assert_eq!(parsed.commit, "abc123");
        assert!(parsed.is_clean());
        assert!(!parsed.is_dirty());
        assert!(!parsed.is_bless());
    }

    #[test]
    fn parse_key_recognizes_a_dirty_snapshot() {
        let parsed = parse_key(
            "v1/folo/objects/criterion/x86_64-pc-windows-msvc/m1/abc123/dirty-1700000000.json",
        )
        .unwrap();
        assert!(parsed.is_dirty());
        assert_eq!(
            parsed.kind,
            ObjectKind::Dirty {
                observation_unix: 1_700_000_000
            }
        );
        assert_eq!(parsed.set.target_triple, "x86_64-pc-windows-msvc");
        assert_eq!(parsed.set.machine_key, "m1");
    }

    #[test]
    fn parse_key_recognizes_a_blessing_sidecar() {
        let parsed = parse_key(
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/abc123/bless-1700000000.json",
        )
        .unwrap();
        assert!(parsed.is_bless());
        assert!(!parsed.is_dirty());
        assert_eq!(
            parsed.kind,
            ObjectKind::Bless {
                issued_unix: 1_700_000_000
            }
        );
        assert_eq!(parsed.commit, "abc123");
    }

    #[test]
    fn bless_key_targets_the_sets_commit_directory() {
        let parsed =
            parse_key("v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/abc123/clean.json")
                .unwrap();
        assert_eq!(
            parsed.bless_key(1_700_000_000),
            "v1/folo/objects/callgrind/x86_64-unknown-linux-gnu/m1/abc123/bless-1700000000.json"
        );
    }

    #[test]
    fn parse_key_rejects_malformed_keys() {
        // Wrong (unrecognized) storage version, even with an otherwise valid shape.
        assert!(parse_key("v2/folo/objects/callgrind/t/m/c/f.json").is_none());
        // Missing the fixed `objects` segment (the pre-objects v1 shape).
        assert!(parse_key("v1/folo/callgrind/t/m/c/f.json").is_none());
        // A different literal in the objects position.
        assert!(parse_key("v1/folo/data/callgrind/t/m/c/f.json").is_none());
        // Structurally malformed keys at the recognized version.
        assert!(parse_key("v1/folo/objects/callgrind/t/m/f.json").is_none());
        assert!(parse_key("v1/folo/objects/callgrind/t/m/c/sub/f.json").is_none());
        assert!(parse_key("v1/folo/objects/callgrind/t//c/f.json").is_none());
        assert!(parse_key("v1/folo/objects/callgrind/t/m/c/").is_none());
        // A structurally valid key naming an engine the tool has no adapter for.
        assert!(parse_key("v1/folo/objects/dhat/t/m/c/clean.json").is_none());
        // A structurally valid key whose file segment is not a known object.
        assert!(parse_key("v1/folo/objects/callgrind/t/m/c/notes.json").is_none());
    }

    #[test]
    fn engine_serde_matches_as_str() {
        // The serialized wire form must equal `as_str`, so a flattened
        // `DiscriminantSet` keeps the exact engine name in JSON reports.
        for engine in Engine::ALL {
            let json = serde_json::to_string(&engine).unwrap();
            assert_eq!(json, format!("\"{}\"", engine.as_str()));
            assert_eq!(
                serde_json::from_str::<Engine>(&json).unwrap(),
                engine,
                "engine {engine} must round-trip through its wire form"
            );
        }
    }
}
