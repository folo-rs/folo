//! Discriminant-filter resolution for benchmark-history queries.
//!
//! This turns the raw `--engine` / `--target-triple` / `--machine-key`
//! selectors and auto-detected [`AutoDiscriminants`] into the
//! [`DiscriminantFilter`]s the object listing applies, then describes them for
//! diagnostics.

use cbh_detect::{DiscriminantFilter, DiscriminantSetQuery};
use cbh_model::Engine;
use nonempty::NonEmpty;

use super::selection::Selection;
use crate::{AnalyzeError, UnknownEngineError};

/// The current machine's auto-detected discriminant values.
///
/// These values are used as defaults when a discriminant filter is omitted (see the
/// *Discriminant sets and discriminant filters* section of `DESIGN.md`).
///
/// Production probes these once (host triple from `rustc -vV`, machine key from
/// the hardware fingerprint); tests pass deterministic literals. There is no auto
/// engine — a bare query analyzes every engine — so only the triple and machine
/// key are detected.
///
/// Exposed so integration tests can inject deterministic values through the
/// binary's `Overrides` test hook, keeping the suite independent of the host it
/// runs on.
#[derive(Clone, Debug)]
pub struct AutoDiscriminants {
    /// The host target triple (`rustc -vV` host).
    pub triple: String,
    /// The host machine fingerprint.
    pub machine_key: String,
}

/// Resolves one discriminant filter's raw command-line values.
///
/// The case-insensitive `all` keyword (anywhere in the list) is an explicit
/// synonym for no filter. An empty list auto-detects: the current-machine value
/// when one is supplied (`auto`), else no filter (engine has no host default).
fn resolve_discriminant(values: &[String], auto: Option<&str>) -> DiscriminantFilter {
    if values.iter().any(|value| value.eq_ignore_ascii_case("all")) {
        return DiscriminantFilter::All;
    }
    match NonEmpty::from_vec(values.to_vec()) {
        Some(values) => DiscriminantFilter::Explicit(values),
        None => auto.map_or(DiscriminantFilter::All, |value| {
            DiscriminantFilter::Auto(value.to_owned())
        }),
    }
}

/// Resolves every command-line discriminant filter into a [`DiscriminantSetQuery`].
///
/// `auto` supplies the current-machine defaults for the triple and machine-key
/// filters when those are omitted. Passing `None` resolves omitted filters to no
/// constraint instead — used by the `discriminants` catalog listing, which is a
/// discovery view over all stored partitions rather than the current machine's.
pub(crate) fn resolve_discriminants(
    selection: &Selection<'_>,
    auto: Option<&AutoDiscriminants>,
) -> Result<DiscriminantSetQuery, AnalyzeError> {
    let engine = resolve_discriminant(selection.engine, None);
    if let DiscriminantFilter::Explicit(values) = &engine {
        for value in values.iter() {
            parse_engine(Some(value))?;
        }
    }
    Ok(DiscriminantSetQuery {
        engine,
        target_triple: resolve_discriminant(
            selection.target_triple,
            auto.map(|a| a.triple.as_str()),
        ),
        machine_key: resolve_discriminant(
            selection.machine_key,
            auto.map(|a| a.machine_key.as_str()),
        ),
    })
}

/// A human-readable summary of the active discriminant filters, for `--verbose` notes.
pub(crate) fn describe_discriminants(discriminants: &DiscriminantSetQuery) -> String {
    let parts = [
        ("engine", &discriminants.engine),
        ("target_triple", &discriminants.target_triple),
        ("machine_key", &discriminants.machine_key),
    ]
    .into_iter()
    .filter_map(|(label, filter)| {
        describe_discriminant_filter(filter).map(|value| format!("{label}={value}"))
    })
    .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

/// Renders one discriminant filter, or `None` when it imposes no constraint.
fn describe_discriminant_filter(filter: &DiscriminantFilter) -> Option<String> {
    match filter {
        DiscriminantFilter::All => None,
        DiscriminantFilter::Auto(value) => Some(format!("{value} (auto-detected)")),
        DiscriminantFilter::Explicit(values) => {
            Some(values.iter().cloned().collect::<Vec<_>>().join("|"))
        }
    }
}

/// Renders every discriminant filter's effective value — always naming all three, including an
/// unconstrained `all` — for the always-on effective-selection summary and the
/// empty-partition hint.
///
/// Unlike [`describe_discriminants`] (a verbose-note helper that omits unconstrained
/// filters), this names each filter so a plain run shows exactly which discriminant
/// partition it searched and which values were auto-detected.
pub(crate) fn describe_effective_discriminants(discriminants: &DiscriminantSetQuery) -> String {
    [
        ("engine", &discriminants.engine),
        ("target-triple", &discriminants.target_triple),
        ("machine-key", &discriminants.machine_key),
    ]
    .into_iter()
    .map(|(label, filter)| format!("{label}={}", describe_effective_discriminant_filter(filter)))
    .collect::<Vec<_>>()
    .join(", ")
}

/// Renders one discriminant filter for the effective summary, naming an unconstrained
/// filter `all` rather than omitting it.
fn describe_effective_discriminant_filter(filter: &DiscriminantFilter) -> String {
    match filter {
        DiscriminantFilter::All => "all".to_owned(),
        DiscriminantFilter::Auto(value) => format!("{value} (auto-detected)"),
        DiscriminantFilter::Explicit(values) => {
            values.iter().cloned().collect::<Vec<_>>().join("|")
        }
    }
}

/// Whether every discriminant filter is unconstrained (`all`): the query spans every stored
/// discriminant partition rather than a specific machine's. Distinguishes a
/// genuinely empty project from an auto-detected partition that matched nothing.
pub(crate) fn discriminants_are_unconstrained(discriminants: &DiscriminantSetQuery) -> bool {
    matches!(discriminants.engine, DiscriminantFilter::All)
        && matches!(discriminants.target_triple, DiscriminantFilter::All)
        && matches!(discriminants.machine_key, DiscriminantFilter::All)
}

/// Parses an `--engine` discriminant value into an [`Engine`], if set.
fn parse_engine(name: Option<&str>) -> Result<Option<Engine>, AnalyzeError> {
    match name {
        None => Ok(None),
        Some(name) => Engine::from_name(name)
            .map(Some)
            .ok_or_else(|| UnknownEngineError::new(name).into()),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_detect::{DiscriminantFilter, DiscriminantSetQuery};
    use nonempty::nonempty;
    use ohno::ErrorExt as _;

    use super::*;

    #[test]
    fn describe_discriminants_joins_set_discriminants_and_reports_none_when_empty() {
        let empty = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::All,
            machine_key: DiscriminantFilter::All,
        };
        assert_eq!(describe_discriminants(&empty), "none");

        let full = DiscriminantSetQuery {
            engine: DiscriminantFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: DiscriminantFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: DiscriminantFilter::Explicit(nonempty!["abcd".to_owned()]),
        };
        assert_eq!(
            describe_discriminants(&full),
            "engine=criterion, target_triple=x86_64-pc-windows-msvc (auto-detected), \
             machine_key=abcd"
        );
    }

    #[test]
    fn parse_engine_resolves_none_known_and_rejects_unknown() {
        assert!(parse_engine(None).unwrap().is_none());
        assert!(parse_engine(Some("callgrind")).unwrap().is_some());
        let error = parse_engine(Some("nonsuch")).unwrap_err();
        let found = error.find_source::<UnknownEngineError>().unwrap();
        assert_eq!(found.name, "nonsuch");
    }

    #[test]
    fn describe_effective_discriminants_names_every_discriminant_including_all() {
        let query = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: DiscriminantFilter::Explicit(nonempty!["abcd".to_owned()]),
        };
        // Unlike `describe_discriminants`, an unconstrained filter is named `all` rather
        // than dropped, so the summary always shows the full partition searched.
        assert_eq!(
            describe_effective_discriminants(&query),
            "engine=all, target-triple=x86_64-pc-windows-msvc (auto-detected), machine-key=abcd"
        );
    }

    #[test]
    fn discriminants_are_unconstrained_only_when_every_discriminant_is_all() {
        let all = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::All,
            machine_key: DiscriminantFilter::All,
        };
        assert!(discriminants_are_unconstrained(&all));

        let one_auto = DiscriminantSetQuery {
            engine: DiscriminantFilter::All,
            target_triple: DiscriminantFilter::Auto("x86_64".to_owned()),
            machine_key: DiscriminantFilter::All,
        };
        assert!(!discriminants_are_unconstrained(&one_auto));

        let one_explicit = DiscriminantSetQuery {
            engine: DiscriminantFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: DiscriminantFilter::All,
            machine_key: DiscriminantFilter::All,
        };
        assert!(!discriminants_are_unconstrained(&one_explicit));
    }
}
