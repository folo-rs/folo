//! Machine fingerprint: a stable hash of the host's hardware characteristics,
//! used to partition hardware-dependent (Criterion) results so that only runs
//! from equivalent machines share a series.
//!
//! The fingerprint must be reproducible across tool versions and across the
//! equivalent machines of a single pool, so it is a fixed SHA-256 over a
//! versioned, canonical string of hardware factors — never a seeded or
//! process-specific hash. Cloud machines in one pool share these factors and so
//! share a key; genuinely different hardware produces a different key. The host
//! *name* is deliberately not a factor, since pool machines have distinct names
//! but equivalent hardware.

use sha2::{Digest, Sha256};

use crate::comparability::sanitize_segment;

/// Version tag of the fingerprint factor set, prefixed onto the canonical string.
///
/// Bumping it deliberately forks the machine key (and thus the stored series) so
/// that a change to which factors are hashed is an explicit, visible event rather
/// than a silent break in history.
const FINGERPRINT_VERSION: &str = "mk1";

/// Number of hex characters kept from the SHA-256 digest. Eight bytes (64 bits)
/// is short enough for a readable path segment yet wide enough that accidental
/// collisions between distinct hardware profiles are not a practical concern.
const FINGERPRINT_HEX_LEN: usize = 16;

/// Hardware facts that identify a machine for partitioning hardware-dependent
/// benchmark results.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct HardwareProfile {
    /// Maximum number of logical processors the system reports.
    pub(crate) processors: usize,
    /// Maximum number of NUMA memory regions the system reports.
    pub(crate) memory_regions: usize,
    /// Best-effort CPU brand string (`None` when it cannot be determined).
    pub(crate) cpu_brand: Option<String>,
}

/// Renders a profile to its canonical, hashable string (pure).
///
/// The factors are emitted in a fixed order as `key=value` lines, prefixed with
/// the fingerprint version. The brand is normalized so incidental whitespace
/// differences do not change the key; an absent brand contributes an empty value
/// so that machines differing only in whether a brand could be read still hash
/// consistently with their factor set.
fn canonical(profile: &HardwareProfile) -> String {
    let brand = profile
        .cpu_brand
        .as_deref()
        .map(normalize_brand)
        .unwrap_or_default();
    format!(
        "{FINGERPRINT_VERSION}\nprocessors={}\nmemory_regions={}\ncpu_brand={brand}",
        profile.processors, profile.memory_regions,
    )
}

/// Collapses runs of whitespace to single spaces and trims the ends, so two
/// identical CPUs whose brand strings differ only cosmetically hash alike (pure).
fn normalize_brand(brand: &str) -> String {
    brand.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stable machine fingerprint of `profile`: the lowercase hex of the first
/// [`FINGERPRINT_HEX_LEN`] characters of the SHA-256 of its canonical string.
pub(crate) fn fingerprint(profile: &HardwareProfile) -> String {
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(canonical(profile).as_bytes());
    let digest = hasher.finalize();

    let bytes_needed = FINGERPRINT_HEX_LEN.div_ceil(2);
    let mut hex = String::with_capacity(FINGERPRINT_HEX_LEN);
    for byte in digest.iter().take(bytes_needed) {
        write!(hex, "{byte:02x}").expect("writing to a String never fails");
    }
    hex.truncate(FINGERPRINT_HEX_LEN);
    hex
}

/// Resolves the machine key for a partition: an explicit override (sanitized to a
/// single path segment) wins; otherwise the hardware [`fingerprint`] is used.
pub(crate) fn resolve_machine_key(override_key: Option<&str>, profile: &HardwareProfile) -> String {
    match override_key {
        Some(key) => sanitize_segment(key),
        None => fingerprint(profile),
    }
}

/// Profiles the host hardware (best effort).
///
/// The processor and memory-region counts come from `many_cpus`; the CPU brand is
/// read per platform and left `None` when it cannot be determined.
#[cfg_attr(test, mutants::skip)] // Queries the host; the pure logic it feeds is tested.
pub(crate) async fn system_profile() -> HardwareProfile {
    let hardware = many_cpus::SystemHardware::current();
    HardwareProfile {
        processors: hardware.max_processor_count(),
        memory_regions: hardware.max_memory_region_count(),
        cpu_brand: detect_cpu_brand().await,
    }
}

#[cfg(target_os = "linux")]
#[cfg_attr(test, mutants::skip)] // Reads `/proc`; the parser it delegates to is tested.
async fn detect_cpu_brand() -> Option<String> {
    let text = tokio::fs::read_to_string("/proc/cpuinfo").await.ok()?;
    parse_cpuinfo_brand(&text)
}

#[cfg(target_os = "windows")]
#[cfg_attr(test, mutants::skip)] // Reads an environment variable; nothing further to assert.
#[expect(
    clippy::unused_async,
    reason = "uniform async signature across the per-platform detect_cpu_brand variants"
)]
async fn detect_cpu_brand() -> Option<String> {
    std::env::var("PROCESSOR_IDENTIFIER")
        .ok()
        .map(|value| normalize_brand(&value))
        .filter(|value| !value.is_empty())
}

#[cfg(target_os = "macos")]
#[cfg_attr(test, mutants::skip)] // Spawns `sysctl`; the parser it delegates to is tested.
async fn detect_cpu_brand() -> Option<String> {
    let output = tokio::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_sysctl_brand(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
#[cfg_attr(test, mutants::skip)] // No CPU-brand source is known on this platform.
#[expect(
    clippy::unused_async,
    reason = "uniform async signature across the per-platform detect_cpu_brand variants"
)]
async fn detect_cpu_brand() -> Option<String> {
    None
}

/// Extracts the CPU brand from `/proc/cpuinfo` text: the value of the first
/// `model name` line, whitespace-normalized (pure).
#[cfg(any(test, target_os = "linux"))]
fn parse_cpuinfo_brand(text: &str) -> Option<String> {
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() == "model name" {
            let brand = normalize_brand(value);
            if !brand.is_empty() {
                return Some(brand);
            }
        }
    }
    None
}

/// Extracts the CPU brand from `sysctl -n machdep.cpu.brand_string` output: the
/// whitespace-normalized text, or `None` when it is empty (pure).
#[cfg(any(test, target_os = "macos"))]
fn parse_sysctl_brand(text: &str) -> Option<String> {
    let brand = normalize_brand(text);
    if brand.is_empty() { None } else { Some(brand) }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn profile(processors: usize, memory_regions: usize, brand: Option<&str>) -> HardwareProfile {
        HardwareProfile {
            processors,
            memory_regions,
            cpu_brand: brand.map(ToOwned::to_owned),
        }
    }

    #[test]
    fn fingerprint_is_stable_for_a_fixed_profile() {
        // A pinned golden value proves the fingerprint is a fixed hash of the
        // canonical factor string, not a seeded or platform-varying one. If the
        // factor set, ordering, version tag, or hash changes, this must be
        // updated deliberately (and existing history is understood to fork).
        let key = fingerprint(&profile(8, 1, Some("Test CPU 3000")));
        assert_eq!(key, "d3ddd69dcf3b84ea");
    }

    #[test]
    fn fingerprint_is_sixteen_lowercase_hex_chars() {
        let key = fingerprint(&profile(4, 2, None));
        assert_eq!(key.len(), FINGERPRINT_HEX_LEN);
        assert!(
            key.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "{key}"
        );
    }

    #[test]
    fn distinct_factors_produce_distinct_fingerprints() {
        let base = fingerprint(&profile(8, 1, Some("Brand A")));
        assert_ne!(base, fingerprint(&profile(16, 1, Some("Brand A"))));
        assert_ne!(base, fingerprint(&profile(8, 2, Some("Brand A"))));
        assert_ne!(base, fingerprint(&profile(8, 1, Some("Brand B"))));
        assert_ne!(base, fingerprint(&profile(8, 1, None)));
    }

    #[test]
    fn brand_whitespace_differences_do_not_change_the_fingerprint() {
        assert_eq!(
            fingerprint(&profile(8, 1, Some("Intel Xeon  E5"))),
            fingerprint(&profile(8, 1, Some("  Intel   Xeon E5 "))),
        );
    }

    #[test]
    fn canonical_is_versioned_and_ordered() {
        let rendered = canonical(&profile(8, 1, Some("CPU X")));
        assert_eq!(
            rendered,
            "mk1\nprocessors=8\nmemory_regions=1\ncpu_brand=CPU X"
        );
    }

    #[test]
    fn canonical_renders_absent_brand_as_empty() {
        let rendered = canonical(&profile(2, 1, None));
        assert!(rendered.ends_with("cpu_brand="), "{rendered}");
    }

    #[test]
    fn normalize_brand_collapses_and_trims_whitespace() {
        assert_eq!(normalize_brand("  a   b\tc \n"), "a b c");
        assert_eq!(normalize_brand("   "), "");
    }

    #[test]
    fn resolve_machine_key_uses_fingerprint_without_override() {
        let hardware = profile(8, 1, Some("CPU X"));
        assert_eq!(resolve_machine_key(None, &hardware), fingerprint(&hardware));
    }

    #[test]
    fn resolve_machine_key_prefers_sanitized_override() {
        let hardware = profile(8, 1, Some("CPU X"));
        assert_eq!(
            resolve_machine_key(Some("ci/pool one"), &hardware),
            "ci_pool_one"
        );
    }

    #[test]
    fn parse_cpuinfo_brand_reads_the_first_model_name() {
        let text = "\
processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel(R) Xeon(R) CPU E5-2673 v4 @ 2.30GHz
processor\t: 1
model name\t: Some Other Core
";
        assert_eq!(
            parse_cpuinfo_brand(text),
            Some("Intel(R) Xeon(R) CPU E5-2673 v4 @ 2.30GHz".to_owned())
        );
    }

    #[test]
    fn parse_cpuinfo_brand_absent_yields_none() {
        assert_eq!(parse_cpuinfo_brand("processor\t: 0\nflags\t: fpu\n"), None);
        assert_eq!(parse_cpuinfo_brand(""), None);
    }

    #[test]
    fn parse_cpuinfo_brand_blank_value_yields_none() {
        assert_eq!(parse_cpuinfo_brand("model name\t:   \n"), None);
    }

    #[test]
    fn parse_sysctl_brand_normalizes_and_handles_empty() {
        assert_eq!(
            parse_sysctl_brand("Apple M2 Pro\n"),
            Some("Apple M2 Pro".to_owned())
        );
        assert_eq!(parse_sysctl_brand("  \n"), None);
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)] // Queries real hardware, which Miri cannot model.
    async fn system_profile_reports_at_least_one_processor() {
        let hardware = system_profile().await;
        assert!(hardware.processors >= 1, "{hardware:?}");
        assert!(hardware.memory_regions >= 1, "{hardware:?}");
        // The fingerprint of whatever this machine is must be well-formed.
        assert_eq!(fingerprint(&hardware).len(), FINGERPRINT_HEX_LEN);
    }
}
