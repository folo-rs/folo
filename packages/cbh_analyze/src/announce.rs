//! The always-on effective-selection announcement shared by every command that
//! auto-detects or defaults a selection parameter.
//!
//! Each query or mutation command resolves the same kinds of hidden input — the
//! discriminant partition (target-triple / machine-key auto-detected), the base
//! branch, the context commit, the `--since` window — and echoes them through one
//! builder here, so the wording is identical everywhere and there is a single
//! formatter to maintain. The line is emitted to stderr regardless of `--verbose`
//! (see [`ReporterExt::announce`](cbh_diag::ReporterExt::announce)), so a plain run
//! never hides a value it defaulted.

use cbh_detect::{DiscriminantSetQuery, FacetFilter};
use cbh_diag::{Reporter, ReporterExt};
use cbh_model::Engine;
use jiff::Timestamp;

use super::facets::describe_effective_facets;

/// The always-on notice explaining that machine-independent (`synthetic`)
/// benchmarks are included alongside this machine's data — or `None` when it does
/// not apply.
///
/// It applies exactly when the machine-key facet was auto-detected *and* the
/// engine facet still admits a synthetic-producing engine (Callgrind or
/// `alloc_tracker`). A `synthetic` set carries no machine key, so it rides along
/// whatever the detected key is — but only the hardware-independent engines
/// produce one, so an explicit `--engine criterion` (or `all_the_time`) filters
/// every synthetic set out and the notice would promise data that is not in
/// scope. Naming the inclusion keeps a plain run from silently mixing in data the
/// user did not obviously ask for, without over-promising when it cannot apply.
pub(crate) fn machine_independent_inclusion_notice(
    facets: &DiscriminantSetQuery,
) -> Option<&'static str> {
    let machine_key_auto = matches!(facets.machine_key, FacetFilter::Auto(_));
    (machine_key_auto && engine_admits_synthetic(&facets.engine)).then_some(
        "Machine-independent benchmarks targeting the 'synthetic' machine key are also \
         included when using auto-detected machine key",
    )
}

/// Whether the engine facet still lets a synthetic-producing engine through.
///
/// The hardware-independent engines — Callgrind and `alloc_tracker` — are the
/// only ones whose sets land in the `synthetic` machine-key partition. `All`
/// admits every engine; an explicit list admits synthetic sets only when it names
/// at least one hardware-independent engine.
fn engine_admits_synthetic(engine: &FacetFilter) -> bool {
    match engine {
        FacetFilter::All => true,
        FacetFilter::Auto(value) => names_synthetic_engine(value),
        FacetFilter::Explicit(values) => values.iter().any(|value| names_synthetic_engine(value)),
    }
}

/// Whether a single engine facet value names a hardware-independent (synthetic)
/// engine.
fn names_synthetic_engine(value: &str) -> bool {
    Engine::from_name(value).is_some_and(|engine| !engine.is_hardware_dependent())
}

/// Emits the always-on effective-selection `summary`, then the
/// machine-independent-inclusion notice when it applies (see
/// [`machine_independent_inclusion_notice`]).
///
/// Every command's selection announcement flows through here so the notice appears
/// on its own line wherever the summary does.
pub(crate) fn announce_selection(
    reporter: &dyn Reporter,
    facets: &DiscriminantSetQuery,
    summary: &str,
) {
    reporter.announce(summary);
    if let Some(notice) = machine_independent_inclusion_notice(facets) {
        reporter.announce(notice);
    }
}

/// The resolved base branch a run split history against, for the announcement.
pub(crate) struct AnnouncedBase<'a> {
    /// The base ref's display name.
    pub(crate) name: &'a str,
    /// Whether the base was auto-detected (no explicit `--base`).
    pub(crate) auto: bool,
}

/// The resolved context commit a command acts at, for the announcement.
pub(crate) struct AnnouncedContext<'a> {
    /// The short commit ID the context resolved to.
    pub(crate) short: &'a str,
    /// Whether the context defaulted to `HEAD` (no explicit `--context`).
    pub(crate) defaulted_head: bool,
}

/// The resolved `--since` cutoff and the reason it holds that value.
pub(crate) struct AnnouncedSince<'a> {
    /// The resolved lower-bound instant, or `None` for no cutoff.
    pub(crate) cutoff: Option<Timestamp>,
    /// Why the cutoff is what it is (e.g. explicit, or a mode default).
    pub(crate) reason: &'a str,
}

/// Builds the always-on, one-line effective-selection announcement.
///
/// Leads with the discriminant partition (always, naming auto-detected facets) and
/// appends whichever of the base branch, context commit, and `--since` window the
/// command resolved — each marked when auto-detected or defaulted — so the caller
/// passes only the segments that apply to it.
pub(crate) fn selection_announcement(
    facets: &DiscriminantSetQuery,
    base: Option<AnnouncedBase<'_>>,
    context: Option<AnnouncedContext<'_>>,
    since: Option<AnnouncedSince<'_>>,
) -> String {
    let mut segments = vec![format!("selection: {}", describe_effective_facets(facets))];
    if let Some(base) = base {
        segments.push(if base.auto {
            format!("base={} (auto-detected)", base.name)
        } else {
            format!("base={}", base.name)
        });
    }
    if let Some(context) = context {
        segments.push(if context.defaulted_head {
            format!("context={} (defaulted to HEAD)", context.short)
        } else {
            format!("context={}", context.short)
        });
    }
    if let Some(since) = since {
        segments.push(format!(
            "since={} ({})",
            since
                .cutoff
                .map_or_else(|| "none".to_owned(), |cutoff| cutoff.to_string()),
            since.reason
        ));
    }
    segments.join("; ")
}

#[cfg(test)]
mod tests {
    use cbh_detect::FacetFilter;
    use nonempty::nonempty;

    use super::*;

    fn auto_facets() -> DiscriminantSetQuery {
        DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Auto("abcd".to_owned()),
        }
    }

    #[test]
    fn facets_only_line_names_just_the_partition() {
        let line = selection_announcement(&auto_facets(), None, None, None);
        assert_eq!(
            line,
            "selection: engine=all, target-triple=x86_64-pc-windows-msvc (auto-detected), \
             machine-key=abcd (auto-detected)"
        );
    }

    #[test]
    fn base_segment_marks_auto_and_explicit() {
        let auto = selection_announcement(
            &auto_facets(),
            Some(AnnouncedBase {
                name: "main",
                auto: true,
            }),
            None,
            None,
        );
        assert!(auto.contains("; base=main (auto-detected)"), "{auto}");

        let explicit = selection_announcement(
            &auto_facets(),
            Some(AnnouncedBase {
                name: "release",
                auto: false,
            }),
            None,
            None,
        );
        assert!(explicit.contains("; base=release"), "{explicit}");
        assert!(!explicit.contains("base=release (auto"), "{explicit}");
    }

    #[test]
    fn context_segment_marks_defaulted_head_and_explicit() {
        let defaulted = selection_announcement(
            &auto_facets(),
            None,
            Some(AnnouncedContext {
                short: "a1b2c3d4",
                defaulted_head: true,
            }),
            None,
        );
        assert!(
            defaulted.contains("; context=a1b2c3d4 (defaulted to HEAD)"),
            "{defaulted}"
        );

        let explicit = selection_announcement(
            &auto_facets(),
            None,
            Some(AnnouncedContext {
                short: "a1b2c3d4",
                defaulted_head: false,
            }),
            None,
        );
        assert!(explicit.contains("; context=a1b2c3d4"), "{explicit}");
        assert!(!explicit.contains("defaulted to HEAD"), "{explicit}");
    }

    #[test]
    fn since_segment_renders_cutoff_and_none_with_reason() {
        let cutoff = Timestamp::from_second(1_700_000_000).unwrap();
        let with_cutoff = selection_announcement(
            &auto_facets(),
            None,
            None,
            Some(AnnouncedSince {
                cutoff: Some(cutoff),
                reason: "from the --since option",
            }),
        );
        assert!(
            with_cutoff.contains(&format!("; since={cutoff} (from the --since option)")),
            "{with_cutoff}"
        );

        let no_cutoff = selection_announcement(
            &auto_facets(),
            None,
            None,
            Some(AnnouncedSince {
                cutoff: None,
                reason: "no default look-back",
            }),
        );
        assert!(
            no_cutoff.contains("; since=none (no default look-back)"),
            "{no_cutoff}"
        );
    }

    #[test]
    fn machine_key_auto_triggers_the_synthetic_inclusion_notice() {
        // Auto-detected machine key: synthetic sets ride along, so the notice fires.
        let auto = DiscriminantSetQuery {
            engine: FacetFilter::All,
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Auto("abcd".to_owned()),
        };
        assert_eq!(
            machine_independent_inclusion_notice(&auto),
            Some(
                "Machine-independent benchmarks targeting the 'synthetic' machine key are also \
                 included when using auto-detected machine key"
            )
        );

        // Explicit or unconstrained machine key: no auto default to explain.
        let explicit = DiscriminantSetQuery {
            machine_key: FacetFilter::Explicit(nonempty!["abcd".to_owned()]),
            ..DiscriminantSetQuery::default()
        };
        assert_eq!(machine_independent_inclusion_notice(&explicit), None);
        assert_eq!(
            machine_independent_inclusion_notice(&DiscriminantSetQuery::default()),
            None
        );
    }

    #[test]
    fn engine_facet_gates_the_synthetic_inclusion_notice() {
        let notice = "Machine-independent benchmarks targeting the 'synthetic' machine key are \
                      also included when using auto-detected machine key";
        let with_engine = |engine: FacetFilter| DiscriminantSetQuery {
            engine,
            target_triple: FacetFilter::Auto("x86_64-pc-windows-msvc".to_owned()),
            machine_key: FacetFilter::Auto("abcd".to_owned()),
        };

        // An engine facet scoped to a hardware-dependent engine filters every
        // synthetic set out, so the notice would over-promise and must not fire.
        assert_eq!(
            machine_independent_inclusion_notice(&with_engine(FacetFilter::Explicit(nonempty![
                "criterion".to_owned()
            ]))),
            None
        );
        assert_eq!(
            machine_independent_inclusion_notice(&with_engine(FacetFilter::Explicit(nonempty![
                "all_the_time".to_owned()
            ]))),
            None
        );

        // A synthetic-producing engine (Callgrind / alloc_tracker), alone or
        // alongside a hardware-dependent one, keeps synthetic sets in scope.
        assert_eq!(
            machine_independent_inclusion_notice(&with_engine(FacetFilter::Explicit(nonempty![
                "callgrind".to_owned()
            ]))),
            Some(notice)
        );
        assert_eq!(
            machine_independent_inclusion_notice(&with_engine(FacetFilter::Explicit(nonempty![
                "alloc_tracker".to_owned()
            ]))),
            Some(notice)
        );
        assert_eq!(
            machine_independent_inclusion_notice(&with_engine(FacetFilter::Explicit(nonempty![
                "criterion".to_owned(),
                "callgrind".to_owned()
            ]))),
            Some(notice)
        );
    }

    #[test]
    fn segments_appear_in_facets_base_context_since_order() {
        let cutoff = Timestamp::from_second(1_700_000_000).unwrap();
        let facets = DiscriminantSetQuery {
            engine: FacetFilter::Explicit(nonempty!["criterion".to_owned()]),
            target_triple: FacetFilter::All,
            machine_key: FacetFilter::All,
        };
        let line = selection_announcement(
            &facets,
            Some(AnnouncedBase {
                name: "main",
                auto: false,
            }),
            Some(AnnouncedContext {
                short: "deadbeef",
                defaulted_head: true,
            }),
            Some(AnnouncedSince {
                cutoff: Some(cutoff),
                reason: "from the --since option",
            }),
        );
        assert_eq!(
            line,
            format!(
                "selection: engine=criterion, target-triple=all, machine-key=all; base=main; \
                 context=deadbeef (defaulted to HEAD); since={cutoff} (from the --since option)"
            )
        );
    }
}
