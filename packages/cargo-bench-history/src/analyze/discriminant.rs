//! Discriminant sets: the comparability boundary `analyze` lists and selects.
//!
//! A *discriminant set* is the `engine / target_triple / machine` triple (within
//! one project) that makes two runs comparable — the segment of a storage key
//! above the commit directory (see DESIGN §4.3). Operating system and CPU
//! architecture are *derived facets* parsed from the target triple so a user can
//! select sets without memorizing triples.

use std::fmt;

/// The comparability boundary a series belongs to, parsed from a storage key.
///
/// Within a single project all runs that share a discriminant set are comparable;
/// runs in different sets (a different engine, target triple, or machine key)
/// never share a series.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
pub(crate) struct DiscriminantSet {
    /// Engine identifier (for example, `callgrind`).
    pub(crate) engine: String,
    /// Resolved target triple the run was recorded under.
    pub(crate) target_triple: String,
    /// Machine partition (`synthetic` for hardware-independent engines).
    pub(crate) machine: String,
}

impl DiscriminantSet {
    /// The operating-system facet derived from the target triple.
    pub(crate) fn os(&self) -> &'static str {
        os_from_triple(&self.target_triple)
    }

    /// The CPU-architecture facet derived from the target triple.
    pub(crate) fn architecture(&self) -> &str {
        arch_from_triple(&self.target_triple)
    }

    /// Whether this set passes every facet of `facets` that is set.
    pub(crate) fn matches(&self, facets: &Facets<'_>) -> bool {
        facet_matches(facets.engine, &self.engine)
            && facet_matches(facets.target_triple, &self.target_triple)
            && facet_matches(facets.os, self.os())
            && facet_matches(facets.architecture, self.architecture())
            && facet_matches(facets.machine_key, &self.machine)
    }
}

/// The facet filters from the command line; each `None` facet is unconstrained.
///
/// `target_triple` matches the whole partition value directly; `os` and
/// `architecture` are the derived facets. A caller must not set `target_triple`
/// together with either derived facet (the triple already fixes both) — that
/// mutual exclusion is enforced before the `Facets` are built (see `analyze`).
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Facets<'a> {
    /// Restrict to a single engine (for example, `callgrind`).
    pub(crate) engine: Option<&'a str>,
    /// Restrict to a single full target triple (for example,
    /// `x86_64-unknown-linux-gnu`).
    pub(crate) target_triple: Option<&'a str>,
    /// Restrict to a single operating system (for example, `windows`).
    pub(crate) os: Option<&'a str>,
    /// Restrict to a single CPU architecture (for example, `x86_64`).
    pub(crate) architecture: Option<&'a str>,
    /// Restrict to a single machine partition.
    pub(crate) machine_key: Option<&'a str>,
}

/// The components a storage key decomposes into.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ParsedKey {
    /// The (sanitized) project segment.
    pub(crate) project: String,
    /// The discriminant set the key belongs to.
    pub(crate) set: DiscriminantSet,
    /// The commit directory segment (full SHA, or `unknown`).
    pub(crate) commit: String,
    /// The file segment (`clean.json` or `dirty-<unix>.json`).
    pub(crate) file: String,
}

impl ParsedKey {
    /// Whether the key names a dirty (uncommitted-tree) snapshot.
    pub(crate) fn is_dirty(&self) -> bool {
        self.file.starts_with("dirty-")
    }
}

/// Parses a storage object key into its components.
///
/// Keys have the form `v2/{project}/{engine}/{triple}/{machine}/{commit}/{file}` —
/// exactly seven non-empty segments. Any key that does not match that shape
/// exactly (wrong version, too few or too many segments, or an empty segment) is
/// ignored (returns `None`) rather than misattributed.
pub(crate) fn parse_key(key: &str) -> Option<ParsedKey> {
    let parts: Vec<&str> = key.split('/').collect();
    let [
        version,
        project,
        engine,
        target_triple,
        machine,
        commit,
        file,
    ] = parts.as_slice()
    else {
        return None;
    };
    if *version != "v2" {
        return None;
    }
    if parts.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    Some(ParsedKey {
        project: (*project).to_owned(),
        set: DiscriminantSet {
            engine: (*engine).to_owned(),
            target_triple: (*target_triple).to_owned(),
            machine: (*machine).to_owned(),
        },
        commit: (*commit).to_owned(),
        file: (*file).to_owned(),
    })
}

/// Whether an optional facet constraint is satisfied by `actual` (case-insensitive).
fn facet_matches(want: Option<&str>, actual: &str) -> bool {
    want.is_none_or(|want| want.eq_ignore_ascii_case(actual))
}

/// Derives the CPU architecture from a target triple (its first component).
fn arch_from_triple(triple: &str) -> &str {
    triple.split('-').next().unwrap_or(triple)
}

/// Derives a normalized operating-system facet from a target triple.
///
/// The OS token is recognized anywhere in the triple so `…-pc-windows-msvc`,
/// `…-unknown-linux-gnu`, and `…-apple-darwin` all map to a stable short name.
/// A triple whose OS token is none of `windows`, `darwin`/`apple`, or `linux`
/// maps to `"unknown"` (so e.g. `freebsd` and `wasm32-unknown-unknown` both
/// surface under the `unknown` OS facet rather than a per-triple value).
fn os_from_triple(triple: &str) -> &'static str {
    if triple.contains("windows") {
        "windows"
    } else if triple.contains("darwin") || triple.contains("apple") {
        "macos"
    } else if triple.contains("linux") {
        "linux"
    } else {
        "unknown"
    }
}

impl fmt::Display for DiscriminantSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}/{}", self.engine, self.target_triple, self.machine)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn set(triple: &str) -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: triple.to_owned(),
            machine: "synthetic".to_owned(),
        }
    }

    #[test]
    fn architecture_is_the_first_component() {
        assert_eq!(set("x86_64-unknown-linux-gnu").architecture(), "x86_64");
        assert_eq!(set("aarch64-apple-darwin").architecture(), "aarch64");
        assert_eq!(
            set("riscv64gc-unknown-linux-gnu").architecture(),
            "riscv64gc"
        );
    }

    #[test]
    fn os_facet_normalizes_each_platform() {
        assert_eq!(set("x86_64-pc-windows-msvc").os(), "windows");
        assert_eq!(set("x86_64-pc-windows-gnu").os(), "windows");
        assert_eq!(set("x86_64-unknown-linux-gnu").os(), "linux");
        assert_eq!(set("aarch64-apple-darwin").os(), "macos");
        // An Apple triple carrying only the `apple` token (no `darwin`) still maps
        // to macos, so the `darwin`/`apple` recognition is a disjunction.
        assert_eq!(set("aarch64-apple-ios").os(), "macos");
        assert_eq!(set("wasm32-unknown-unknown").os(), "unknown");
    }

    #[test]
    fn parse_key_decomposes_a_clean_key() {
        let parsed =
            parse_key("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/clean.json")
                .expect("a well-formed key parses");
        assert_eq!(parsed.project, "folo");
        assert_eq!(parsed.set.engine, "callgrind");
        assert_eq!(parsed.set.target_triple, "x86_64-unknown-linux-gnu");
        assert_eq!(parsed.set.machine, "synthetic");
        assert_eq!(parsed.commit, "abc123");
        assert_eq!(parsed.file, "clean.json");
        assert!(!parsed.is_dirty());
    }

    #[test]
    fn parse_key_recognizes_a_dirty_snapshot() {
        let parsed =
            parse_key("v2/folo/criterion/x86_64-pc-windows-msvc/m1/abc123/dirty-1700000000.json")
                .expect("a well-formed dirty key parses");
        assert!(parsed.is_dirty());
        assert_eq!(parsed.set.os(), "windows");
        assert_eq!(parsed.set.machine, "m1");
    }

    #[test]
    fn parse_key_rejects_malformed_keys() {
        assert!(parse_key("v1/folo/callgrind/t/m/c/f.json").is_none());
        assert!(parse_key("v2/folo/callgrind/t/m/f.json").is_none());
        assert!(parse_key("v2/folo/callgrind/t/m/c/sub/f.json").is_none());
        assert!(parse_key("v2//callgrind/t/m/c/f.json").is_none());
        assert!(parse_key("v2/folo/callgrind/t/m/c/").is_none());
    }

    #[test]
    fn matches_requires_every_set_facet() {
        let windows = set("x86_64-pc-windows-msvc");
        assert!(windows.matches(&Facets {
            os: Some("windows"),
            architecture: Some("x86_64"),
            ..Facets::default()
        }));
        // Case-insensitive on the user-facing facets.
        assert!(windows.matches(&Facets {
            os: Some("Windows"),
            ..Facets::default()
        }));
        assert!(!windows.matches(&Facets {
            os: Some("linux"),
            ..Facets::default()
        }));
        assert!(!windows.matches(&Facets {
            engine: Some("criterion"),
            ..Facets::default()
        }));
        assert!(!windows.matches(&Facets {
            machine_key: Some("m1"),
            ..Facets::default()
        }));
    }

    #[test]
    fn matches_on_the_full_target_triple() {
        let linux = set("x86_64-unknown-linux-gnu");
        assert!(linux.matches(&Facets {
            target_triple: Some("x86_64-unknown-linux-gnu"),
            ..Facets::default()
        }));
        // Case-insensitive, like the other user-facing facets.
        assert!(linux.matches(&Facets {
            target_triple: Some("X86_64-Unknown-Linux-Gnu"),
            ..Facets::default()
        }));
        // A triple that shares the architecture but not the whole value misses.
        assert!(!linux.matches(&Facets {
            target_triple: Some("x86_64-pc-windows-msvc"),
            ..Facets::default()
        }));
    }

    #[test]
    fn display_joins_the_three_components() {
        assert_eq!(
            set("x86_64-unknown-linux-gnu").to_string(),
            "callgrind/x86_64-unknown-linux-gnu/synthetic"
        );
    }
}
