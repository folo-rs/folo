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
    #[must_use]
    pub fn new(
        project: String,
        system: EngineSystem,
        target_triple: String,
        machine_key: Option<String>,
    ) -> Self {
        Self {
            project,
            system,
            target_triple,
            machine_key,
        }
    }

    /// The storage prefix that all runs in this series share.
    ///
    /// Layout: `v1/{project}/{system}/{target_triple}/{machine_key|synthetic}`.
    #[must_use]
    pub fn partition_prefix(&self) -> String {
        let project = &self.project;
        let system = self.system.as_str();
        let triple = &self.target_triple;
        let machine = self.machine_key.as_deref().unwrap_or("synthetic");
        format!("v1/{project}/{system}/{triple}/{machine}")
    }

    /// The full object key for one run within this series.
    ///
    /// Named by **effective** time so that backfilled points sort into their
    /// historical position: `{prefix}/{effective_unix}-{short_commit}-{run_id}.json`.
    #[must_use]
    pub fn object_key(&self, effective_unix: i64, short_commit: &str, run_id: &str) -> String {
        let prefix = self.partition_prefix();
        format!("{prefix}/{effective_unix}-{short_commit}-{run_id}.json")
    }
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
            "folo".to_owned(),
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu".to_owned(),
            None,
        );
        assert_eq!(
            key.partition_prefix(),
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic"
        );
    }

    #[test]
    fn machine_key_appears_in_partition() {
        let key = ComparabilityKey::new(
            "folo".to_owned(),
            EngineSystem::Criterion,
            "x86_64-pc-windows-msvc".to_owned(),
            Some("abc123".to_owned()),
        );
        assert_eq!(
            key.partition_prefix(),
            "v1/folo/criterion/x86_64-pc-windows-msvc/abc123"
        );
    }

    #[test]
    fn object_key_is_named_by_effective_time() {
        let key = ComparabilityKey::new(
            "folo".to_owned(),
            EngineSystem::Callgrind,
            "x86_64-unknown-linux-gnu".to_owned(),
            None,
        );
        assert_eq!(
            key.object_key(1_700_000_000, "deadbee", "run-7"),
            "v1/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/1700000000-deadbee-run-7.json"
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
}
