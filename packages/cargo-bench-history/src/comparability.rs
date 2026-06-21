//! Comparability: deciding which runs may be compared to each other, and how
//! the storage is partitioned so that only comparable runs share a series.
//!
//! The guiding rule (see DESIGN §4) is to partition only by what makes results
//! *fundamentally* incomparable — project, engine system, target triple, and (for
//! hardware-dependent engines) a machine key — and to record everything else as
//! metadata so its effect stays visible in the timeline.

use std::fmt;

/// A benchmark engine, distinguished by whether its results depend on hardware.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum EngineSystem {
    /// Criterion wall-clock benchmarks: hardware-dependent and noisy.
    Criterion,
    /// Callgrind (via Gungraun) instruction counts: simulated, hardware-independent.
    Callgrind,
}

impl EngineSystem {
    /// Every supported engine, in a stable order used to inject the combined
    /// benchmark environment and to harvest each engine's output tree after the
    /// single `cargo bench` invocation.
    pub const ALL: [Self; 2] = [Self::Callgrind, Self::Criterion];

    /// The stable lowercase identifier used in storage paths and config keys.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Criterion => "criterion",
            Self::Callgrind => "callgrind",
        }
    }

    /// Parses a [`EngineSystem`] from its stable lowercase identifier.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "criterion" => Some(Self::Criterion),
            "callgrind" => Some(Self::Callgrind),
            _ => None,
        }
    }

    /// Whether results from this engine depend on the host hardware.
    ///
    /// Hardware-dependent engines require a machine key in their partition;
    /// hardware-independent ones use the literal `synthetic` instead.
    #[must_use]
    pub fn is_hardware_dependent(self) -> bool {
        match self {
            Self::Criterion => true,
            Self::Callgrind => false,
        }
    }
}

impl fmt::Display for EngineSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolves the target triple to record for a run (see DESIGN §4.1).
///
/// Resolution order (first match wins):
///
/// 1. An explicit triple (from `--target-triple` or per-engine config).
/// 2. For Callgrind, the OS component is pinned to `linux` because the engine only
///    runs under Valgrind — this transparently handles the Windows→WSL case.
/// 3. Otherwise (natively-run engines such as Criterion) the tool's host triple,
///    whose architecture a WSL guest shares.
#[must_use]
pub fn resolve_target_triple(
    explicit: Option<&str>,
    engine: EngineSystem,
    host_triple: &str,
) -> String {
    if let Some(triple) = explicit {
        return triple.to_owned();
    }
    match engine {
        EngineSystem::Callgrind => normalize_os_to_linux(host_triple),
        EngineSystem::Criterion => host_triple.to_owned(),
    }
}

/// Pins the OS component of a triple to a Linux/GNU target.
///
/// If the triple already names Linux it is kept verbatim; otherwise only the
/// architecture (the first component) is retained and recombined with
/// `-unknown-linux-gnu`, reflecting that a WSL guest shares the host architecture.
fn normalize_os_to_linux(host_triple: &str) -> String {
    if host_triple.contains("linux") {
        return host_triple.to_owned();
    }
    let arch = host_triple.split('-').next().unwrap_or(host_triple);
    format!("{arch}-unknown-linux-gnu")
}

/// The set of factors that must match for two runs to share a series, and which
/// therefore form the storage partition.
#[non_exhaustive]
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ComparabilityKey {
    /// Workspace/project identity.
    pub project: String,
    /// The benchmark engine system.
    pub system: EngineSystem,
    /// Resolved target triple (see [`resolve_target_triple`]).
    pub target_triple: String,
    /// Machine fingerprint for hardware-dependent engines; `None` otherwise.
    pub machine_key: Option<String>,
}

impl ComparabilityKey {
    /// Creates a comparability key.
    ///
    /// The path-forming components (`project`, `target_triple`, `machine_key`) are
    /// sanitized so that every segment is a single, well-formed path component:
    /// any character that is not ASCII alphanumeric, `-`, `_`, or `.` is replaced
    /// with `_`, and a segment that would otherwise be empty or consist only of
    /// dots becomes `_`. This keeps a stray `/` (or other surprising input) from
    /// silently splitting the storage key into the wrong number of segments —
    /// which `analyze` would then fail to attribute — by mangling the value rather
    /// than rejecting the run.
    #[must_use]
    pub fn new(
        project: &str,
        system: EngineSystem,
        target_triple: &str,
        machine_key: Option<&str>,
    ) -> Self {
        Self {
            project: sanitize_segment(project),
            system,
            target_triple: sanitize_segment(target_triple),
            machine_key: machine_key.map(sanitize_segment),
        }
    }

    /// The storage prefix that all runs in this series share.
    ///
    /// Layout: `v2/{project}/{system}/{target_triple}/{machine_key|synthetic}`.
    /// Below this prefix the history is organized by commit (see [`clean_key`] and
    /// [`dirty_key`]) so `analyze` can resolve a series from git topology.
    ///
    /// [`clean_key`]: Self::clean_key
    /// [`dirty_key`]: Self::dirty_key
    #[must_use]
    pub fn partition_prefix(&self) -> String {
        let project = &self.project;
        let system = self.system.as_str();
        let triple = &self.target_triple;
        let machine = self.machine_key.as_deref().unwrap_or("synthetic");
        format!("v2/{project}/{system}/{triple}/{machine}")
    }

    /// The object key for the canonical (clean working tree) result at `commit`.
    ///
    /// Layout: `{prefix}/{commit}/clean.json`. A clean run is keyed solely by its
    /// commit, so it is deterministic: a second clean run of the same commit maps
    /// to the same key and collides, which the write-once storage detects so `run`
    /// can refuse the duplicate unless an overwrite is explicitly requested.
    ///
    /// `commit` is sanitized the same way as the partition components so the
    /// directory name always forms a single key segment.
    #[must_use]
    pub fn clean_key(&self, commit: &str) -> String {
        let prefix = self.partition_prefix();
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/clean.json")
    }

    /// The object key for a dirty (uncommitted-changes) snapshot at `commit`,
    /// taken at `effective_unix`.
    ///
    /// Layout: `{prefix}/{commit}/dirty-{effective_unix}.json`. Because a dirty
    /// snapshot does not correspond to committed code, it is distinguished by its
    /// effective time rather than by the commit alone, so multiple dirty snapshots
    /// on the same base commit coexist; only two snapshots sharing an effective
    /// second collide.
    ///
    /// `commit` is sanitized the same way as the partition components so the
    /// directory name always forms a single key segment.
    #[must_use]
    pub fn dirty_key(&self, commit: &str, effective_unix: i64) -> String {
        let prefix = self.partition_prefix();
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/dirty-{effective_unix}.json")
    }

    /// The object key for a blessing sidecar at `commit`, issued at `issued_unix`.
    ///
    /// Layout: `{prefix}/{commit}/bless-{issued_unix}.json`. Blessings are
    /// append-only sidecars distinguished by their issue time, so several coexist
    /// on one commit and are unioned at query time.
    ///
    /// `commit` is sanitized the same way as the partition components so the
    /// directory name always forms a single key segment.
    #[must_use]
    pub fn bless_key(&self, commit: &str, issued_unix: i64) -> String {
        let prefix = self.partition_prefix();
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/bless-{issued_unix}.json")
    }

    /// The storage prefix shared by every object recorded at `commit` in this
    /// partition (`{prefix}/{commit}/`), used to enumerate a commit directory.
    ///
    /// `commit` is sanitized the same way as the partition components so the
    /// directory name always forms a single key segment.
    #[must_use]
    pub fn commit_prefix(&self, commit: &str) -> String {
        let prefix = self.partition_prefix();
        let commit = sanitize_segment(commit);
        format!("{prefix}/{commit}/")
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
pub(crate) fn sanitize_segment(raw: &str) -> String {
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
    fn explicit_triple_wins() {
        let triple = resolve_target_triple(
            Some("custom-triple"),
            EngineSystem::Callgrind,
            "x86_64-pc-windows-msvc",
        );
        assert_eq!(triple, "custom-triple");
    }

    #[test]
    fn callgrind_pins_windows_host_to_linux() {
        let triple = resolve_target_triple(None, EngineSystem::Callgrind, "x86_64-pc-windows-msvc");
        assert_eq!(triple, "x86_64-unknown-linux-gnu");
    }

    #[test]
    fn callgrind_keeps_linux_host_verbatim() {
        let triple =
            resolve_target_triple(None, EngineSystem::Callgrind, "aarch64-unknown-linux-gnu");
        assert_eq!(triple, "aarch64-unknown-linux-gnu");
    }

    #[test]
    fn criterion_uses_host_triple() {
        let triple = resolve_target_triple(None, EngineSystem::Criterion, "x86_64-pc-windows-msvc");
        assert_eq!(triple, "x86_64-pc-windows-msvc");
    }

    #[test]
    fn hardware_dependence_matches_engine() {
        assert!(EngineSystem::Criterion.is_hardware_dependent());
        assert!(!EngineSystem::Callgrind.is_hardware_dependent());
    }

    #[test]
    fn synthetic_partition_for_hardware_independent_engine() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        assert_eq!(
            key.partition_prefix(),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic"
        );
    }

    #[test]
    fn machine_key_appears_in_partition() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Criterion,
            "x86_64-pc-windows-msvc",
            Some("abc123"),
        );
        assert_eq!(
            key.partition_prefix(),
            "v2/folo/criterion/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn clean_key_is_named_by_commit() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        assert_eq!(
            key.clean_key("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef"),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/clean.json"
        );
    }

    #[test]
    fn dirty_key_is_named_by_commit_and_effective_time() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        assert_eq!(
            key.dirty_key("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", 1_700_000_000),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/dirty-1700000000.json"
        );
    }

    #[test]
    fn bless_key_is_named_by_commit_and_issue_time() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        assert_eq!(
            key.bless_key("deadbeefdeadbeefdeadbeefdeadbeefdeadbeef", 1_700_000_000),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/\
             deadbeefdeadbeefdeadbeefdeadbeefdeadbeef/bless-1700000000.json"
        );
    }

    #[test]
    fn commit_prefix_enumerates_one_commit_directory() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        assert_eq!(
            key.commit_prefix("dead/beef"),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/"
        );
    }

    #[test]
    fn engine_system_display_matches_as_str() {
        assert_eq!(EngineSystem::Criterion.to_string(), "criterion");
        assert_eq!(EngineSystem::Callgrind.to_string(), "callgrind");
    }

    #[test]
    fn engine_system_from_name_roundtrips() {
        for engine in [EngineSystem::Criterion, EngineSystem::Callgrind] {
            assert_eq!(EngineSystem::from_name(engine.as_str()), Some(engine));
        }
        assert_eq!(EngineSystem::from_name("dhat"), None);
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
        let key = ComparabilityKey::new(
            "team/app",
            EngineSystem::Criterion,
            "weird/triple",
            Some("machine/one"),
        );
        assert_eq!(
            key.partition_prefix(),
            "v2/team_app/criterion/weird_triple/machine_one"
        );
        // The partition prefix has exactly the five canonical segments.
        assert_eq!(key.partition_prefix().split('/').count(), 5);
    }

    #[test]
    fn clean_key_sanitizes_the_commit() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        let object = key.clean_key("dead/beef");
        assert_eq!(
            object,
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/clean.json"
        );
        // Exactly the seven canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 7);
    }

    #[test]
    fn dirty_key_sanitizes_the_commit() {
        let key = ComparabilityKey::new(
            "folo",
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu",
            None,
        );
        let object = key.dirty_key("dead/beef", 1_700_000_000);
        assert_eq!(
            object,
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/dead_beef/dirty-1700000000.json"
        );
        // Exactly the seven canonical key segments survive sanitization.
        assert_eq!(object.split('/').count(), 7);
    }
}
