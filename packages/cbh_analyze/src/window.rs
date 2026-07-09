//! `--since` / `--until` window resolution: parsing the edges, deciding the
//! history-mode default look-back, and testing each committer time against the
//! resolved window.

use cbh_analysis::AnalysisMode;
use cbh_run::RunError;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use jiff::{Span, Timestamp};

/// Auto-detects the analysis mode from the resolved topology and recorded data.
///
/// An official base-branch view — the target's tip *is* its own merge-base with
/// the base and no dirty run is recorded on that tip — is
/// [`AnalysisMode::History`]. Anything else (commits past the merge-base, or a
/// dirty run actually recorded on top of the base tip) is treated as an unnamed
/// feature branch: [`AnalysisMode::Branch`]. `auto_mode` reads only these two
/// signals; the working tree enters only indirectly, since a base-tip dirty run
/// counts as feature-branch data only while the tree is currently dirty — a dirty
/// checkout with no admitted dirty run still analyzes as history. The merge-base
/// is always known here: an undeterminable one is a hard error in `resolve_history`,
/// never a silent fallback to this mode.
pub(crate) fn auto_mode(tip_is_merge_base: bool, dirty_tip_run_present: bool) -> AnalysisMode {
    if tip_is_merge_base && !dirty_tip_run_present {
        AnalysisMode::History
    } else {
        AnalysisMode::Branch
    }
}

/// Explains, for a verbose note, why the resolved `--since` cutoff is what it is.
pub(crate) fn since_cutoff_reason(explicit_since: bool, mode: AnalysisMode) -> &'static str {
    if explicit_since {
        "from the --since option"
    } else if mode == AnalysisMode::History {
        "history-mode default six-month look-back"
    } else {
        "no default look-back window outside history mode"
    }
}

/// Resolves the effective `--since` cutoff: an explicit value always wins;
/// otherwise history mode applies a default look-back so a scheduled trend watch
/// does not silently widen as history accumulates, while branch mode has no default
/// (a feature branch's whole history is in scope).
pub(crate) fn resolve_since(
    value: Option<&str>,
    mode: AnalysisMode,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    if value.is_some() {
        return parse_since(value, now);
    }
    if mode == AnalysisMode::History {
        return default_history_since(now).map(Some);
    }
    Ok(None)
}

/// Default history-mode look-back window: six months before `now`.
const HISTORY_DEFAULT_LOOKBACK_MONTHS: i32 = 6;

/// The instant [`HISTORY_DEFAULT_LOOKBACK_MONTHS`] before `now`, anchored with
/// calendar-correct zoned arithmetic (months have no fixed length).
fn default_history_since(now: Timestamp) -> Result<Timestamp, RunError> {
    now.to_zoned(TimeZone::UTC)
        .checked_sub(Span::new().months(HISTORY_DEFAULT_LOOKBACK_MONTHS))
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("default --since window is out of the representable range: {error}"),
        })
}

/// Parses the `--since` option into an absolute lower-bound instant, if set.
///
/// See [`parse_instant`] for the accepted input forms. `now` anchors a relative
/// duration.
pub(crate) fn parse_since(
    value: Option<&str>,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    parse_instant(value, "--since", now)
}

/// Parses the `--until` option into an absolute upper-bound instant, if set.
///
/// See [`parse_instant`] for the accepted input forms. `now` anchors a relative
/// duration.
pub(crate) fn parse_until(
    value: Option<&str>,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    parse_instant(value, "--until", now)
}

/// Which edge of the `--since`/`--until` window a commit falls outside of.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum WindowEdge {
    /// The commit predates the `--since` lower bound.
    Since,
    /// The commit postdates the `--until` upper bound.
    Until,
}

/// Decides whether a commit's `committer_time` places it outside the
/// `--since`/`--until` window, reporting which edge it fell off.
///
/// `--since` is an inclusive-on-or-after lower bound and `--until` an
/// inclusive-on-or-before upper bound, matching the object-timestamp comparison
/// the analysis used before topology carried the time. A commit with an unknown
/// time (`None`) is never excluded — git always reports a committer date for a
/// real commit, so this only guards a degenerate input conservatively (by
/// keeping the object and letting the rest of the pipeline judge it).
pub(crate) fn window_excludes(
    committer_time: Option<Timestamp>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> Option<WindowEdge> {
    let time = committer_time?;
    if since.is_some_and(|since| time < since) {
        return Some(WindowEdge::Since);
    }
    if until.is_some_and(|until| time > until) {
        return Some(WindowEdge::Until);
    }
    None
}

/// Parses a time-cutoff option into an absolute instant, if set.
///
/// Three input forms are accepted, tried in order:
///
/// * an RFC 3339 timestamp (`2024-01-01T00:00:00Z`),
/// * a bare `YYYY-MM-DD` date, interpreted at UTC midnight, and
/// * a relative duration in jiff's friendly or ISO 8601 form (`5 months`,
///   `5 months ago`, `P6M`, `2w`), interpreted as *that far in the past* —
///   resolved against `now` via calendar-correct zoned arithmetic.
///
/// `now` is the analysis anchor sourced from the injected clock (see
/// [`resolve_now`]), so the relative form is deterministic under a frozen clock
/// rather than reading the wall clock afresh.
///
/// The relative form is normalized through [`Span::abs`] before subtracting, so a
/// duration written with the friendly `ago` suffix (which jiff parses as a
/// *negative* span) still means "this far back" rather than flipping into the
/// future. A cutoff in the future is never a sensible bound, so both `5 months`
/// and `5 months ago` resolve to the same past instant.
fn parse_instant(
    value: Option<&str>,
    flag: &str,
    now: Timestamp,
) -> Result<Option<Timestamp>, RunError> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.trim();
    if let Ok(timestamp) = value.parse::<Timestamp>() {
        return Ok(Some(timestamp));
    }
    if let Ok(date) = value.parse::<Date>() {
        // UTC has no DST transitions, so civil midnight always maps to an instant.
        let zoned = date
            .to_zoned(TimeZone::UTC)
            .expect("UTC midnight is always a valid instant");
        return Ok(Some(zoned.timestamp()));
    }
    if let Ok(span) = value.parse::<Span>() {
        return Ok(Some(instant_before(span, flag, now)?));
    }
    Err(RunError::Analyze {
        message: format!(
            "invalid {flag} value {value:?}; expected an RFC 3339 timestamp, a YYYY-MM-DD \
             date, or a relative duration such as \"6 months\" or \"30 days ago\""
        ),
    })
}

/// Resolves a relative [`Span`] to the instant that far before `now`, treating the
/// span's magnitude as a look-back regardless of its sign (see [`parse_instant`]).
///
/// Calendar units (months, years) have no fixed length, so the subtraction is
/// anchored to `now`'s UTC zoned datetime rather than to a bare duration.
fn instant_before(span: Span, flag: &str, now: Timestamp) -> Result<Timestamp, RunError> {
    now.to_zoned(TimeZone::UTC)
        .checked_sub(span.abs())
        .map(|zoned| zoned.timestamp())
        .map_err(|error| RunError::Analyze {
            message: format!("{flag} duration is out of the representable range: {error}"),
        })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use cbh_analysis::AnalysisMode;
    use cbh_run::RunError;
    use jiff::Timestamp;

    use super::*;

    fn ts(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).unwrap()
    }

    #[test]
    fn auto_mode_uses_tip_topology_and_recorded_dirty_runs_not_repo_state() {
        // A base branch whose tip is its own merge-base with no dirty run recorded
        // on the tip is history mode.
        assert_eq!(auto_mode(true, false), AnalysisMode::History);
        // Commits past the merge-base, or a dirty run actually recorded on top of
        // the tip, make it a (possibly unnamed) feature branch.
        assert_eq!(auto_mode(false, false), AnalysisMode::Branch);
        assert_eq!(auto_mode(true, true), AnalysisMode::Branch);
        assert_eq!(auto_mode(false, true), AnalysisMode::Branch);
    }

    #[test]
    fn since_cutoff_reason_explains_each_source() {
        assert_eq!(
            since_cutoff_reason(true, AnalysisMode::History),
            "from the --since option"
        );
        assert_eq!(
            since_cutoff_reason(true, AnalysisMode::Branch),
            "from the --since option"
        );
        assert_eq!(
            since_cutoff_reason(false, AnalysisMode::History),
            "history-mode default six-month look-back"
        );
        assert_eq!(
            since_cutoff_reason(false, AnalysisMode::Branch),
            "no default look-back window outside history mode"
        );
    }

    #[test]
    fn resolve_since_applies_a_default_only_in_history_mode() {
        let now = "2024-06-01T00:00:00Z".parse::<Timestamp>().unwrap();
        // History mode without an explicit value falls back to the six-month window.
        let history = resolve_since(None, AnalysisMode::History, now)
            .unwrap()
            .unwrap();
        assert_eq!(
            history,
            "2023-12-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
        // Branch mode has no default cutoff.
        assert_eq!(
            resolve_since(None, AnalysisMode::Branch, now).unwrap(),
            None
        );
        // An explicit value always wins, even in branch mode.
        let explicit = resolve_since(Some("2024-01-01"), AnalysisMode::Branch, now)
            .unwrap()
            .unwrap();
        assert_eq!(
            explicit,
            "2024-01-01T00:00:00Z".parse::<Timestamp>().unwrap()
        );
    }

    #[test]
    fn default_history_since_subtracts_six_calendar_months() {
        let now = "2024-03-31T00:00:00Z".parse::<Timestamp>().unwrap();
        // Calendar arithmetic, not a fixed number of days: six months before
        // 2024-03-31 is 2023-09-30 (September has 30 days).
        let cutoff = default_history_since(now).unwrap();
        assert_eq!(cutoff, "2023-09-30T00:00:00Z".parse::<Timestamp>().unwrap());
    }

    #[test]
    fn window_excludes_decides_each_edge_from_committer_time() {
        let since = Some(ts(10));
        let until = Some(ts(20));
        // Before the lower bound, after the upper bound, and inside the window.
        assert_eq!(
            window_excludes(Some(ts(5)), since, until),
            Some(WindowEdge::Since)
        );
        assert_eq!(
            window_excludes(Some(ts(25)), since, until),
            Some(WindowEdge::Until)
        );
        assert_eq!(window_excludes(Some(ts(15)), since, until), None);
        // Both bounds are inclusive: a commit exactly on an edge stays in-window.
        assert_eq!(window_excludes(Some(ts(10)), since, until), None);
        assert_eq!(window_excludes(Some(ts(20)), since, until), None);
        // An open bound never excludes on that side.
        assert_eq!(window_excludes(Some(ts(0)), None, until), None);
        assert_eq!(window_excludes(Some(ts(99)), since, None), None);
        // An unknown committer time is never excluded, even with both bounds set.
        assert_eq!(window_excludes(None, since, until), None);
    }

    #[test]
    fn since_accepts_timestamp_and_date() {
        let now: Timestamp = "2024-06-01T00:00:00Z".parse().unwrap();
        assert_eq!(
            parse_since(Some("2024-01-01T00:00:00Z"), now).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(
            parse_since(Some("2024-01-01"), now).unwrap(),
            Some("2024-01-01T00:00:00Z".parse().unwrap())
        );
        assert_eq!(parse_since(None, now).unwrap(), None);
    }

    #[test]
    fn since_accepts_relative_durations_as_look_back() {
        // A friendly duration resolves to an instant in the past, and the `ago`
        // suffix (which jiff parses as a negative span) means the same look-back
        // rather than flipping into the future. Anchored to a fixed `now`, both
        // spellings land on exactly the same instant — five calendar months
        // before 2024-06-01 is 2024-01-01.
        let now: Timestamp = "2024-06-01T00:00:00Z".parse().unwrap();
        let expected: Timestamp = "2024-01-01T00:00:00Z".parse().unwrap();
        let plain = parse_since(Some("5 months"), now).unwrap().unwrap();
        let with_ago = parse_since(Some("5 months ago"), now).unwrap().unwrap();
        assert_eq!(plain, expected, "5 months before 2024-06-01 is 2024-01-01");
        assert_eq!(
            with_ago, expected,
            "the `ago` suffix looks back the same amount"
        );
        assert!(plain < now, "a look-back must be in the past");
    }

    #[test]
    fn since_accepts_iso_and_week_durations() {
        let now: Timestamp = "2024-06-01T00:00:00Z".parse().unwrap();
        for input in ["P6M", "2w", "30 days", "-P1Y"] {
            let cutoff = parse_since(Some(input), now).unwrap().unwrap();
            assert!(cutoff < now, "{input:?} must resolve to the past");
        }
    }

    #[test]
    fn since_rejects_garbage() {
        let now: Timestamp = "2024-06-01T00:00:00Z".parse().unwrap();
        let error = parse_since(Some("not-a-date"), now).unwrap_err();
        assert!(matches!(error, RunError::Analyze { .. }), "{error:?}");
    }
}
