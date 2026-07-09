//! Discriminant-set facet resolution: turning the raw `--engine` / `--target-triple`
//! / `--machine-key` selectors and the auto-detected `AutoFacets` into the
//! `FacetFilter`s the object listing applies, and describing them for the verbose trail.

use crate::AnalyzeError;
use cbh_detect::{DiscriminantSetQuery, FacetFilter};
use cbh_model::Engine;
use nonempty::NonEmpty;

use super::selection::Selection;

/// The current machine's auto-detected facet values, used as the default when a
/// query facet is omitted (see the *Discriminant set & query facets* section of
/// `DESIGN.md`).
///
/// Production probes these once (host triple from `rustc -vV`, machine key from
/// the hardware fingerprint); tests pass deterministic literals. There is no auto
/// engine — a bare query analyzes every engine — so only the triple and machine
/// key are detected.
#[derive(Clone, Debug)]
pub(crate) struct AutoFacets {
    /// The host target triple (`rustc -vV` host).
    pub(crate) triple: String,
    /// The host machine fingerprint.
    pub(crate) machine_key: String,
}

/// Resolves one facet's raw command-line values into a [`FacetFilter`].
///
/// The case-insensitive `all` keyword (anywhere in the list) is an explicit
/// synonym for no filter. An empty list auto-detects: the current-machine value
/// when one is supplied (`auto`), else no filter (engine has no host default).
fn resolve_facet(values: &[String], auto: Option<&str>) -> FacetFilter {
    if values.iter().any(|value| value.eq_ignore_ascii_case("all")) {
        return FacetFilter::All;
    }
    match NonEmpty::from_vec(values.to_vec()) {
        Some(values) => FacetFilter::Explicit(values),
        None => auto.map_or(FacetFilter::All, |value| {
            FacetFilter::Auto(value.to_owned())
        }),
    }
}

/// Resolves every command-line facet into a [`DiscriminantSetQuery`] filter, validating that any
/// explicit `--engine` values name a known engine.
///
/// `auto` supplies the current-machine defaults for the triple and machine-key
/// facets when those are omitted. Passing `None` resolves omitted facets to no
/// filter instead — used by the `discriminants` catalog listing, which is a
/// discovery view over all stored partitions rather than the current machine's.
pub(crate) fn resolve_facets(
    selection: &Selection<'_>,
    auto: Option<&AutoFacets>,
) -> Result<DiscriminantSetQuery, AnalyzeError> {
    let engine = resolve_facet(selection.engine, None);
    if let FacetFilter::Explicit(values) = &engine {
        for value in values.iter() {
            parse_engine(Some(value))?;
        }
    }
    Ok(DiscriminantSetQuery {
        engine,
        target_triple: resolve_facet(selection.target_triple, auto.map(|a| a.triple.as_str())),
        machine_key: resolve_facet(selection.machine_key, auto.map(|a| a.machine_key.as_str())),
    })
}

/// A human-readable summary of the active facet filters, for `--verbose` notes.
pub(crate) fn describe_facets(facets: &DiscriminantSetQuery) -> String {
    let parts = [
        ("engine", &facets.engine),
        ("target_triple", &facets.target_triple),
        ("machine_key", &facets.machine_key),
    ]
    .into_iter()
    .filter_map(|(label, filter)| describe_filter(filter).map(|value| format!("{label}={value}")))
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Renders one facet filter, or `None` when it imposes no constraint.
fn describe_filter(filter: &FacetFilter) -> Option<String> {
    match filter {
        FacetFilter::All => None,
        FacetFilter::Auto(value) => Some(format!("{value} (auto-detected)")),
        FacetFilter::Explicit(values) => Some(values.iter().cloned().collect::<Vec<_>>().join("|")),
    }
}

/// Parses an `--engine` facet value into an [`Engine`], if set.
fn parse_engine(name: Option<&str>) -> Result<Option<Engine>, AnalyzeError> {
    match name {
        None => Ok(None),
        Some(name) => Engine::from_name(name)
            .map(Some)
            .ok_or_else(|| AnalyzeError::Analyze {
                message: format!(
                    "unknown engine {name:?}; expected one of: criterion, callgrind, \
                     alloc_tracker, all_the_time"
                ),
            }),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_detect::{DiscriminantSetQuery, FacetFilter};
    use nonempty::nonempty;

    use super::*;

    #[test]
    fn describe_facets_joins_set_facets_and_reports_none_when_empty() {
        let empty = DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::All,
            machine_key: FacetFilter::All,
        };
        assert_eq!(describe_facets(&empty), "none");

        let full = DiscriminantSetQuery {
            engine: FacetFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Explicit(nonempty!["abcd".to_owned()]),
        };
        assert_eq!(
            describe_facets(&full),
            "engine=criterion, target_triple=x86_64-pc-windows-msvc (auto-detected), \
             machine_key=abcd"
        );
    }

    #[test]
    fn parse_engine_resolves_none_known_and_rejects_unknown() {
        assert!(parse_engine(None).unwrap().is_none());
        assert!(parse_engine(Some("callgrind")).unwrap().is_some());
        let error = parse_engine(Some("nonsuch")).unwrap_err();
        assert!(error.to_string().contains("unknown"), "{error}");
    }
}
