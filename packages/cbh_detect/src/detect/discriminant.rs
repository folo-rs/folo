//! The query side of discriminant sets: the facets `analyze`/`list`/`prune`
//! filter on.
//!
//! A *discriminant set* is the `engine / target_triple / machine` triple (within
//! one project) that makes two runs comparable — the segment of a storage key
//! above the commit directory (see the *Discriminant set & query facets* section
//! of `DESIGN.md`). The [`DiscriminantSet`] data-model type — and the
//! [`parse_key`](cbh_model::parse_key) that recovers one from a stored object's
//! key — live in `cbh_model`; this module only adds the read-side concern of
//! filtering on facets.

use cbh_model::DiscriminantSet;
use nonempty::NonEmpty;

/// A resolved filter for one discriminant facet (engine, target triple, or
/// machine key).
///
/// The variant records how the value was supplied so [`DiscriminantSetQuery::matches`]
/// can distinguish an auto-detected default from a deliberate user scope.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum FacetFilter {
    /// Unconstrained: every set passes. Produced by the `all` keyword, and by
    /// `--engine`'s omitted default (there is no host engine to auto-detect to).
    #[default]
    All,
    /// The auto-detected current-machine value, used when the facet is omitted.
    Auto(String),
    /// Explicit user-provided values; a set passes if it equals one of them
    /// (case-insensitive). Never empty — an omitted facet resolves to
    /// [`All`](Self::All) or [`Auto`](Self::Auto) instead, so the values are a
    /// [`NonEmpty`].
    Explicit(NonEmpty<String>),
}

impl FacetFilter {
    /// Whether `actual` passes this filter.
    ///
    /// `exempt` marks a set that ignores this facet entirely (used for the
    /// machine-key facet on a `synthetic` set — `synthetic` means "no machine",
    /// so its results belong to every machine's universe regardless of whether
    /// the facet was auto-detected or explicit).
    fn passes(&self, actual: &str, exempt: bool) -> bool {
        if exempt {
            return true;
        }
        match self {
            Self::All => true,
            Self::Auto(value) => value.eq_ignore_ascii_case(actual),
            Self::Explicit(values) => values
                .iter()
                .any(|value| value.eq_ignore_ascii_case(actual)),
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
    /// Restrict to one or more machine keys.
    pub machine_key: FacetFilter,
}

impl DiscriminantSetQuery {
    /// Whether `set` passes every facet filter.
    ///
    /// Hardware-independent (`synthetic`) sets — Callgrind and `alloc_tracker` —
    /// are exempt from the machine-key facet entirely: their results are not tied
    /// to any one machine, so they belong to every machine's universe. They still
    /// obey the target-triple facet, because even hardware-independent counts are
    /// not comparable across architectures (a per-architecture instruction count
    /// or allocation profile is a different measurement).
    #[must_use]
    pub fn matches(&self, set: &DiscriminantSet) -> bool {
        let synthetic = set.is_synthetic();
        self.engine.passes(&set.engine, false)
            && self.target_triple.passes(&set.target_triple, false)
            && self.machine_key.passes(&set.machine_key, synthetic)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use nonempty::nonempty;

    use super::*;

    fn set(triple: &str) -> DiscriminantSet {
        DiscriminantSet {
            engine: "callgrind".to_owned(),
            target_triple: triple.to_owned(),
            machine_key: "synthetic".to_owned(),
        }
    }

    #[test]
    fn matches_requires_every_set_facet() {
        let windows = set("x86_64-pc-windows-msvc");
        // Explicit target-triple + machine pass.
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(nonempty![
                    "x86_64-pc-windows-msvc".to_owned()
                ]),
                machine_key: FacetFilter::Explicit(nonempty!["synthetic".to_owned()]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // Case-insensitive on the explicit values.
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(nonempty![
                    "X86_64-PC-Windows-MSVC".to_owned()
                ]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // A different explicit triple misses.
        assert!(
            !DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(nonempty![
                    "x86_64-unknown-linux-gnu".to_owned()
                ]),
                ..DiscriminantSetQuery::default()
            }
            .matches(&windows)
        );
        // A different explicit engine misses.
        assert!(
            !DiscriminantSetQuery {
                engine: FacetFilter::Explicit(nonempty!["criterion".to_owned()]),
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
                machine_key: FacetFilter::Explicit(nonempty!["some-other-machine".to_owned()]),
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
    fn synthetic_set_obeys_the_target_triple_facet() {
        let synthetic = set("x86_64-unknown-linux-gnu"); // machine = synthetic
        // A matching auto-detected triple includes it.
        assert!(
            DiscriminantSetQuery {
                target_triple: FacetFilter::Auto("x86_64-unknown-linux-gnu".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
        // A different auto-detected triple excludes it: even hardware-independent
        // counts are not comparable across architectures.
        assert!(
            !DiscriminantSetQuery {
                target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
                ..DiscriminantSetQuery::default()
            }
            .matches(&synthetic)
        );
        // An explicit non-matching triple also excludes it.
        assert!(
            !DiscriminantSetQuery {
                target_triple: FacetFilter::Explicit(nonempty![
                    "x86_64-pc-windows-msvc".to_owned()
                ]),
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
            machine_key: "m1".to_owned(),
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
            target_triple: FacetFilter::Explicit(nonempty![
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
