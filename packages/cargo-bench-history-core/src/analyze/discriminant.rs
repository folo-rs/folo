//! The query side of discriminant sets: the facets `analyze`/`list`/`prune`
//! filter on, and the parser that decomposes a storage key back into its
//! [`DiscriminantSet`] (defined in `comparability`).
//!
//! A *discriminant set* is the `engine / target_triple / machine` triple (within
//! one project) that makes two runs comparable — the segment of a storage key
//! above the commit directory (see the *Discriminant set & query facets* section
//! of `DESIGN.md`). The data-model type lives in `comparability` because the same
//! value is written by `run` and read back here; this module only adds the
//! read-side concerns: filtering on facets and parsing keys.

use crate::comparability::DiscriminantSet;

/// A resolved filter for one discriminant facet (engine, target triple, or
/// machine key).
///
/// The variant records how the value was supplied so [`DiscriminantSetQuery::matches`]
/// can apply the hardware-independent exemption only where intended.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FacetFilter {
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
/// This is the query counterpart of [`DiscriminantSet`]: the set is the data
/// model that `run` writes and `analyze` reads, while the query selects which
/// sets a command operates on. `target_triple` matches the whole partition value
/// directly (operating system and CPU architecture are not separately
/// selectable — filter on the triple).
#[derive(Clone, Debug, Default)]
pub struct DiscriminantSetQuery {
    /// Restrict to one or more engines (for example, `callgrind`).
    pub engine: FacetFilter,
    /// Restrict to one or more full target triples (for example,
    /// `x86_64-unknown-linux-gnu`).
    pub target_triple: FacetFilter,
    /// Restrict to one or more machine partitions.
    pub machine_key: FacetFilter,
}

impl DiscriminantSetQuery {
    /// Whether `set` passes every facet filter.
    ///
    /// Hardware-independent (`synthetic`) sets — Callgrind and `alloc_tracker` —
    /// are exempt from the machine-key facet entirely (their results belong to
    /// every machine's universe) and from an *auto-detected* target-triple facet
    /// (the recorded triple is an artifact, e.g. Callgrind pins Linux). An
    /// explicit `--target-triple` still filters them, as the user asked for that
    /// precise slice.
    #[must_use]
    pub fn matches(&self, set: &DiscriminantSet) -> bool {
        let synthetic = set.is_synthetic();
        self.engine.passes(&set.engine, false, false)
            && self
                .target_triple
                .passes(&set.target_triple, synthetic, false)
            && self.machine_key.passes(&set.machine, synthetic, true)
    }
}

/// The components a storage key decomposes into.
///
/// A storage key references one of three kinds of object in a commit directory:
/// a clean run (`clean.json`), a dirty snapshot (`dirty-<unix>.json`), or a
/// blessing sidecar (`bless-<unix>.json`). The [`file`](Self::file) segment
/// distinguishes them; [`is_dirty`](Self::is_dirty) and
/// [`is_bless`](Self::is_bless) classify it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageKey {
    /// The (sanitized) project segment.
    pub project: String,
    /// The discriminant set the key belongs to.
    pub set: DiscriminantSet,
    /// The commit directory segment (full SHA, or `unknown`).
    pub commit: String,
    /// The file segment (`clean.json`, `dirty-<unix>.json`, or `bless-<unix>.json`).
    pub file: String,
}

impl StorageKey {
    /// Whether the key names a dirty (uncommitted-tree) snapshot.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.file.starts_with("dirty-")
    }

    /// Whether the key names a blessing sidecar rather than a stored run.
    #[must_use]
    pub fn is_bless(&self) -> bool {
        self.file.starts_with("bless-")
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
/// Keys have the form `v2/{project}/{engine}/{triple}/{machine}/{commit}/{file}` —
/// exactly seven non-empty segments. Any key that does not match that shape
/// exactly (wrong version, too few or too many segments, or an empty segment) is
/// ignored (returns `None`) rather than misattributed.
#[must_use]
pub fn parse_key(key: &str) -> Option<StorageKey> {
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
    Some(StorageKey {
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
    fn parse_key_decomposes_a_clean_key() {
        let parsed =
            parse_key("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/clean.json")
                .unwrap();
        assert_eq!(parsed.project, "folo");
        assert_eq!(parsed.set.engine, "callgrind");
        assert_eq!(parsed.set.target_triple, "x86_64-unknown-linux-gnu");
        assert_eq!(parsed.set.machine, "synthetic");
        assert_eq!(parsed.commit, "abc123");
        assert_eq!(parsed.file, "clean.json");
        assert!(!parsed.is_dirty());
        assert!(!parsed.is_bless());
    }

    #[test]
    fn parse_key_recognizes_a_dirty_snapshot() {
        let parsed =
            parse_key("v2/folo/criterion/x86_64-pc-windows-msvc/m1/abc123/dirty-1700000000.json")
                .unwrap();
        assert!(parsed.is_dirty());
        assert_eq!(parsed.set.target_triple, "x86_64-pc-windows-msvc");
        assert_eq!(parsed.set.machine, "m1");
    }

    #[test]
    fn parse_key_recognizes_a_blessing_sidecar() {
        let parsed = parse_key(
            "v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/bless-1700000000.json",
        )
        .unwrap();
        assert!(parsed.is_bless());
        assert!(!parsed.is_dirty());
        assert_eq!(parsed.commit, "abc123");
    }

    #[test]
    fn bless_key_targets_the_sets_commit_directory() {
        let parsed =
            parse_key("v2/folo/callgrind/x86_64-unknown-linux-gnu/synthetic/abc123/clean.json")
                .unwrap();
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
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(vec!["x86_64-pc-windows-msvc".to_owned()]),
                machine_key: FacetFilter::Explicit(vec!["synthetic".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // Case-insensitive on the explicit values.
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(vec!["X86_64-PC-Windows-MSVC".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // A different explicit triple misses.
        assert!(
            !DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(vec!["x86_64-unknown-linux-gnu".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // A different explicit engine misses.
        assert!(
            !DiscriminantSetQuery {
                engine: FacetFilter::Explicit(vec!["criterion".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
    }

    #[test]
    fn synthetic_set_is_exempt_from_the_machine_key_facet() {
        let synthetic = set("x86_64-unknown-linux-gnu"); // machine = synthetic
        // Even an explicit, non-matching machine key includes a synthetic set.
        assert!(
            DiscriminantSetQuery {
                machine_key: FacetFilter::Explicit(vec!["some-other-machine".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
        // And an auto-detected host fingerprint includes it too.
        assert!(
            DiscriminantSetQuery {
                machine_key: FacetFilter::Auto("host-fingerprint".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
    }

    #[test]
    fn synthetic_set_is_exempt_from_an_auto_detected_triple_but_not_an_explicit_one() {
        let synthetic = set("x86_64-unknown-linux-gnu"); // machine = synthetic
        // An auto-detected (omitted) triple does not hide engine-independent data.
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
        // An explicit triple is a deliberate scope and filters even synthetic sets.
        assert!(
            !DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(vec!["x86_64-pc-windows-msvc".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
    }

    #[test]
    fn hardware_dependent_set_obeys_the_auto_detected_machine_key() {
        let machine = DiscriminantSet {
            engine: "criterion".to_owned(),
            target_triple: "x86_64-unknown-linux-gnu".to_owned(),
            machine: "m1".to_owned(),
        };
        // This machine matches its own auto-detected fingerprint.
        assert!(
            DiscriminantSetQuery {
                machine_key: FacetFilter::Auto("m1".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&machine)
        );
        // Another machine's auto-detected fingerprint excludes it.
        assert!(
            !DiscriminantSetQuery {
                machine_key: FacetFilter::Auto("m2".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&machine)
        );
    }

    #[test]
    fn repeated_facet_values_union() {
        let linux = set("x86_64-unknown-linux-gnu");
        let windows = set("x86_64-pc-windows-msvc");
        let either = DiscriminantSetQuery {
            target_triple: FacetFilter::Explicit(vec![
                "x86_64-unknown-linux-gnu".to_owned(),
                "x86_64-pc-windows-msvc".to_owned(),
            ]),
            ..DiscriminantSetQuery::default()
        };
        assert!(either.matches(&linux));
        assert!(either.matches(&windows));
    }

    #[test]
    fn all_filter_matches_every_set() {
        let windows = set("x86_64-pc-windows-msvc");
        assert!(DiscriminantSetQuery::default().matches(&windows));
    }
}
