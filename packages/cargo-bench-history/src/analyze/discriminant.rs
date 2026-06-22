//! Discriminant sets: the comparability boundary `analyze` lists and selects.
//!
//! A *discriminant set* is the `engine / target_triple / machine` triple (within
//! one project) that makes two runs comparable — the segment of a storage key
//! above the commit directory (see DESIGN §4.3). Operating system and CPU
//! architecture are *derived facets* parsed from the target triple, kept only for
//! display in reports and listings (they are no longer selectable filters).

use std::fmt;

/// The comparability boundary a series belongs to, parsed from a storage key.
///
/// Within a single project all runs that share a discriminant set are comparable;
/// runs in different sets (a different engine, target triple, or machine key)
/// never share a series.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Serialize)]
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

    /// Whether this set passes every facet filter.
    ///
    /// Hardware-independent (`synthetic`) sets — Callgrind and `alloc_tracker` —
    /// are exempt from the machine-key facet entirely (their results belong to
    /// every machine's universe) and from an *auto-detected* target-triple facet
    /// (the recorded triple is an artifact, e.g. Callgrind pins Linux). An
    /// explicit `--target-triple` still filters them, as the user asked for that
    /// precise slice.
    pub(crate) fn matches(&self, facets: &Facets) -> bool {
        let synthetic = self.machine == "synthetic";
        facets.engine.passes(&self.engine, false, false)
            && facets
                .target_triple
                .passes(&self.target_triple, synthetic, false)
            && facets.machine_key.passes(&self.machine, synthetic, true)
    }
}

/// A resolved filter for one discriminant facet (engine, target triple, or
/// machine key).
///
/// The variant records how the value was supplied so [`DiscriminantSet::matches`]
/// can apply the hardware-independent exemption only where intended.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) enum FacetFilter {
    /// Unconstrained: every set passes. Produced by the `all` keyword, and by
    /// `--engine`'s omitted default (there is no host engine to auto-detect to).
    #[default]
    All,
    /// The auto-detected current-machine value, used when the facet is omitted.
    /// A `synthetic` set is exempt, so a bare query still surfaces
    /// engine-independent history alongside this machine's data.
    Auto(String),
    /// Explicit user-provided values; a set passes if it equals one of them
    /// (case-insensitive).
    Explicit(Vec<String>),
}

impl FacetFilter {
    /// Whether `actual` passes this filter.
    ///
    /// `synthetic` marks a hardware-independent set; `exempt_explicit` controls
    /// whether such a set is also exempt from explicit values (true for the
    /// machine-key facet — `synthetic` means "no machine" — false for the target
    /// triple, where an explicit value is a deliberate scope).
    fn passes(&self, actual: &str, synthetic: bool, exempt_explicit: bool) -> bool {
        match self {
            Self::All => true,
            Self::Auto(value) => synthetic || value.eq_ignore_ascii_case(actual),
            Self::Explicit(values) => {
                (exempt_explicit && synthetic)
                    || values
                        .iter()
                        .any(|value| value.eq_ignore_ascii_case(actual))
            }
        }
    }
}

/// The facet filters from the command line, each resolved to a [`FacetFilter`].
///
/// `target_triple` matches the whole partition value directly; the derived `os`
/// and `architecture` facets were removed (filter on the triple directly).
#[derive(Clone, Debug, Default)]
pub(crate) struct Facets {
    /// Restrict to one or more engines (for example, `callgrind`).
    pub(crate) engine: FacetFilter,
    /// Restrict to one or more full target triples (for example,
    /// `x86_64-unknown-linux-gnu`).
    pub(crate) target_triple: FacetFilter,
    /// Restrict to one or more machine partitions.
    pub(crate) machine_key: FacetFilter,
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

    /// Whether the key names a blessing sidecar rather than a stored run.
    pub(crate) fn is_bless(&self) -> bool {
        self.file.starts_with("bless-")
    }

    /// The blessing sidecar key for this set's commit directory, issued at
    /// `issued_unix`.
    ///
    /// Layout: `v2/{project}/{engine}/{triple}/{machine}/{commit}/bless-{unix}.json`.
    /// The components are already sanitized (they came from a parsed storage key),
    /// so the result is a well-formed seven-segment key.
    pub(crate) fn bless_key(&self, issued_unix: i64) -> String {
        format!(
            "v2/{}/{}/{}/{}/{}/bless-{issued_unix}.json",
            self.project, self.set.engine, self.set.target_triple, self.set.machine, self.commit,
        )
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
    fn parse_key_recognizes_a_blessing_sidecar() {
        let parsed = parse_key(
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/bless-1700000000.json",
        )
        .expect("a well-formed bless key parses");
        assert!(parsed.is_bless());
        assert!(!parsed.is_dirty());
        assert_eq!(parsed.commit, "abc123");
    }

    #[test]
    fn bless_key_targets_the_sets_commit_directory() {
        let parsed =
            parse_key("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/clean.json")
                .expect("a well-formed key parses");
        assert_eq!(
            parsed.bless_key(1_700_000_000),
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/bless-1700000000.json"
        );
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
        // Explicit target-triple + machine pass.
        assert!(windows.matches(&Facets {
            target_triple: FacetFilter::Explicit(vec!["x86_64-pc-windows-msvc".to_owned()]),
            machine_key: FacetFilter::Explicit(vec!["synthetic".to_owned()]),
            ..Facets::default()
        }));
        // Case-insensitive on the explicit values.
        assert!(windows.matches(&Facets {
            target_triple: FacetFilter::Explicit(vec!["X86_64-PC-Windows-MSVC".to_owned()]),
            ..Facets::default()
        }));
        // A different explicit triple misses.
        assert!(!windows.matches(&Facets {
            target_triple: FacetFilter::Explicit(vec!["x86_64-unknown-linux-gnu".to_owned()]),
            ..Facets::default()
        }));
        // A different explicit engine misses.
        assert!(!windows.matches(&Facets {
            engine: FacetFilter::Explicit(vec!["criterion".to_owned()]),
            ..Facets::default()
        }));
    }

    #[test]
    fn synthetic_set_is_exempt_from_the_machine_key_facet() {
        let synthetic = set("x86_64-unknown-linux-gnu"); // machine = synthetic
        // Even an explicit, non-matching machine key includes a synthetic set.
        assert!(synthetic.matches(&Facets {
            machine_key: FacetFilter::Explicit(vec!["some-other-machine".to_owned()]),
            ..Facets::default()
        }));
        // And an auto-detected host fingerprint includes it too.
        assert!(synthetic.matches(&Facets {
            machine_key: FacetFilter::Auto("host-fingerprint".to_owned()),
            ..Facets::default()
        }));
    }

    #[test]
    fn synthetic_set_is_exempt_from_an_auto_detected_triple_but_not_an_explicit_one() {
        let synthetic = set("x86_64-unknown-linux-gnu"); // machine = synthetic
        // An auto-detected (omitted) triple does not hide engine-independent data.
        assert!(synthetic.matches(&Facets {
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            ..Facets::default()
        }));
        // An explicit triple is a deliberate scope and filters even synthetic sets.
        assert!(!synthetic.matches(&Facets {
            target_triple: FacetFilter::Explicit(vec!["x86_64-pc-windows-msvc".to_owned()]),
            ..Facets::default()
        }));
    }

    #[test]
    fn hardware_dependent_set_obeys_the_auto_detected_machine_key() {
        let machine = DiscriminantSet {
            engine: "criterion".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine: "m1".to_owned(),
        };
        // This machine matches its own auto-detected fingerprint.
        assert!(machine.matches(&Facets {
            machine_key: FacetFilter::Auto("m1".to_owned()),
            ..Facets::default()
        }));
        // Another machine's auto-detected fingerprint excludes it.
        assert!(!machine.matches(&Facets {
            machine_key: FacetFilter::Auto("m2".to_owned()),
            ..Facets::default()
        }));
    }

    #[test]
    fn repeated_facet_values_union() {
        let linux = set("x86_64-unknown-linux-gnu");
        let windows = set("x86_64-pc-windows-msvc");
        let either = Facets {
            target_triple: FacetFilter::Explicit(vec![
                "x86_64-unknown-linux-gnu".to_owned(),
                "x86_64-pc-windows-msvc".to_owned(),
            ]),
            ..Facets::default()
        };
        assert!(linux.matches(&either));
        assert!(windows.matches(&either));
    }

    #[test]
    fn all_filter_matches_every_set() {
        let windows = set("x86_64-pc-windows-msvc");
        assert!(windows.matches(&Facets::default()));
    }

    #[test]
    fn display_joins_the_three_components() {
        assert_eq!(
            set("x86_64-unknown-linux-gnu").to_string(),
            "callgrind/x86_64-unknown-linux-gnu/synthetic"
        );
    }
}
