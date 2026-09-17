//! The report and collection evidence a publisher must validate before writing.

use std::collections::BTreeSet;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;
use serde::Deserialize;

use crate::model::CommitSha;

/// Validated evidence for selecting a report message and permitting all-clear updates.
#[derive(Clone, Debug)]
pub(crate) struct Evidence {
    pub(crate) report: AnalysisReport,
    pub(crate) platforms: PlatformCoverage,
}

impl Evidence {
    pub(crate) fn require_state(&self, state: PublicationState) -> Result<(), AppError> {
        if self.publication_state() != state {
            return Err(WrongPublicationState::new().into());
        }
        Ok(())
    }

    pub(crate) fn is_all_clear(&self) -> bool {
        self.publication_state() == PublicationState::Clean
    }

    pub(crate) fn publication_state(&self) -> PublicationState {
        match self.report.outcome {
            Outcome::Findings => PublicationState::Findings,
            Outcome::Clean
                if self.report.coverage == Coverage::Full && self.platforms.is_complete() =>
            {
                PublicationState::Clean
            }
            _ => PublicationState::NoData,
        }
    }
}

/// The GitHub lifecycle projection of successful analysis and collection evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PublicationState {
    Findings,
    Clean,
    NoData,
}

impl PublicationState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Findings => "findings",
            Self::Clean => "clean",
            Self::NoData => "no-data",
        }
    }
}

/// The supported metadata subset of the tool's JSON, not its findings vocabulary.
#[derive(Clone, Debug)]
pub(crate) struct AnalysisReport {
    pub(crate) commit: CommitSha,
    pub(crate) mode: AnalysisMode,
    pub(crate) outcome: Outcome,
    pub(crate) coverage: Coverage,
}

impl AnalysisReport {
    pub(crate) fn parse(json: &str, expected_commit: &CommitSha) -> Result<Self, AppError> {
        let raw: ReportInput = serde_json::from_str(json).map_err(InvalidReportJson::caused_by)?;
        let commit: CommitSha = raw.tip_commit.parse()?;
        if commit != *expected_commit || raw.tip_dirty {
            return Err(MismatchedReportCommit::new().into());
        }
        if raw.notable != (raw.outcome == Outcome::Findings) {
            return Err(InconsistentReport::new().into());
        }

        let census = &raw.census;
        let counts_match = match census.coverage {
            Coverage::NoSeries | Coverage::NothingInScope => {
                census.judged == 0 && census.in_scope == 0
            }
            Coverage::NothingJudged => census.judged == 0 && census.in_scope > 0,
            Coverage::Partial => census.judged > 0 && census.judged < census.in_scope,
            Coverage::Full => census.judged > 0 && census.judged == census.in_scope,
        };
        let verdict_matches = match raw.outcome {
            Outcome::Findings => census.judged > 0,
            Outcome::Clean => census.coverage == Coverage::Full,
            Outcome::InsufficientBaseline => census.coverage == Coverage::NothingJudged,
            Outcome::NothingInScope => {
                matches!(
                    census.coverage,
                    Coverage::NoSeries | Coverage::NothingInScope
                )
            }
            Outcome::Partial => census.coverage == Coverage::Partial,
        };
        if !counts_match || !verdict_matches {
            return Err(InconsistentReport::new().into());
        }
        Ok(Self {
            commit,
            mode: raw.mode,
            outcome: raw.outcome,
            coverage: census.coverage,
        })
    }

    pub(crate) fn require_mode(&self, mode: AnalysisMode) -> Result<(), AppError> {
        if self.mode != mode {
            return Err(WrongAnalysisMode::new().into());
        }
        Ok(())
    }
}

/// Successful tool verdicts; failed executions never supply a report to publish.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    Findings,
    Clean,
    InsufficientBaseline,
    NothingInScope,
    Partial,
}

impl Outcome {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Findings => "findings",
            Self::Clean => "clean",
            Self::InsufficientBaseline => "insufficient_baseline",
            Self::NothingInScope => "nothing_in_scope",
            Self::Partial => "partial",
        }
    }
}

/// Analysis mode is checked to keep history and PR lifecycle commands distinct.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AnalysisMode {
    History,
    Branch,
}

/// Coverage is separate from findings so neither suppresses the other.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Coverage {
    NoSeries,
    NothingInScope,
    NothingJudged,
    Partial,
    Full,
}

/// Successful collection platforms and their shortfall relative to the requested matrix.
#[derive(Clone, Debug)]
pub(crate) struct PlatformCoverage {
    completed: Vec<String>,
    missing: Vec<String>,
}

impl PlatformCoverage {
    pub(crate) fn parse(expected: &str, completed: &str) -> Result<Self, AppError> {
        let expected = platform_list(expected)?;
        let completed = platform_list(completed)?;
        if !completed.is_subset(&expected) {
            return Err(UnknownCompletedPlatform::new().into());
        }
        let missing = expected.difference(&completed).cloned().collect();
        Ok(Self {
            completed: completed.into_iter().collect(),
            missing,
        })
    }

    pub(crate) fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    pub(crate) fn completed(&self) -> &[String] {
        &self.completed
    }

    pub(crate) fn missing(&self) -> &[String] {
        &self.missing
    }
}

pub(crate) fn platform_list(input: &str) -> Result<BTreeSet<String>, AppError> {
    input
        .split(',')
        .map(str::trim)
        .map(|value| {
            // These are workflow matrix identifiers, not runs-on label arrays or
            // free-form prose. A nonempty identifier is safe to render as inline code.
            if value.is_empty()
                || !value
                    .bytes()
                    .all(|one| one.is_ascii_alphanumeric() || matches!(one, b'.' | b'_' | b'-'))
            {
                return Err(InvalidPlatformList::new().into());
            }
            Ok(value.to_owned())
        })
        .collect()
}

/// JSON decoding is separated from the validated publication contract.
#[derive(Deserialize)]
struct ReportInput {
    tip_commit: String,
    tip_dirty: bool,
    mode: AnalysisMode,
    outcome: Outcome,
    notable: bool,
    census: CensusInput,
}

/// Only the tool's declared coverage facts are needed; reasons remain tool-rendered prose.
#[derive(Deserialize)]
struct CensusInput {
    coverage: Coverage,
    in_scope: usize,
    judged: usize,
}

/// Publication must not infer a missing or unknown tool outcome.
#[ohno::error]
#[display("Report JSON is missing or has unsupported analysis metadata")]
pub(crate) struct InvalidReportJson;

/// A report must describe the committed code the workflow intended to analyze.
#[ohno::error]
#[display("Report commit does not match the requested clean commit")]
pub(crate) struct MismatchedReportCommit;

/// Mutually inconsistent verdict and coverage fields cannot justify publication.
#[ohno::error]
#[display("Report outcome, notable flag and coverage census disagree")]
pub(crate) struct InconsistentReport;

/// The issue and PR commands require different analysis modes.
#[ohno::error]
#[display("Report analysis mode does not match the publication command")]
pub(crate) struct WrongAnalysisMode;

/// Explicit publication commands must agree with the supplied evidence.
#[ohno::error]
#[display("Publication state does not match the validated report and platform coverage")]
pub(crate) struct WrongPublicationState;

/// Successful reports need explicit, nonempty collection evidence.
#[ohno::error]
#[display("Platform lists must contain comma-separated nonempty matrix identifiers")]
pub(crate) struct InvalidPlatformList;

/// A result from an unrequested platform indicates mismatched workflow artifacts.
#[ohno::error]
#[display("A completed platform is absent from the expected platform list")]
pub(crate) struct UnknownCompletedPlatform;

// These immutable diagnostic leaves retain sources but mutate no state during unwinding.
impl UnwindSafe for InvalidReportJson {}
impl RefUnwindSafe for InvalidReportJson {}
impl UnwindSafe for MismatchedReportCommit {}
impl RefUnwindSafe for MismatchedReportCommit {}
impl UnwindSafe for InconsistentReport {}
impl RefUnwindSafe for InconsistentReport {}
impl UnwindSafe for WrongAnalysisMode {}
impl RefUnwindSafe for WrongAnalysisMode {}
impl UnwindSafe for WrongPublicationState {}
impl RefUnwindSafe for WrongPublicationState {}
impl UnwindSafe for InvalidPlatformList {}
impl RefUnwindSafe for InvalidPlatformList {}
impl UnwindSafe for UnknownCompletedPlatform {}
impl RefUnwindSafe for UnknownCompletedPlatform {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Evidence: Send, Sync, UnwindSafe, RefUnwindSafe);

    pub(crate) fn evidence(mode: AnalysisMode, outcome: Outcome, complete: bool) -> Evidence {
        let (coverage, judged, in_scope) = match outcome {
            Outcome::Findings | Outcome::Clean => ("full", 1, 1),
            Outcome::InsufficientBaseline => ("nothing_judged", 0, 1),
            Outcome::NothingInScope => ("no_series", 0, 0),
            Outcome::Partial => ("partial", 1, 2),
        };
        let mode = match mode {
            AnalysisMode::History => "history",
            AnalysisMode::Branch => "branch",
        };
        let outcome_name = match outcome {
            Outcome::Findings => "findings",
            Outcome::Clean => "clean",
            Outcome::InsufficientBaseline => "insufficient_baseline",
            Outcome::NothingInScope => "nothing_in_scope",
            Outcome::Partial => "partial",
        };
        let commit: CommitSha = "a".repeat(40).parse().unwrap();
        let raw = serde_json::json!({
            "tip_commit": commit.as_str(), "tip_dirty": false, "mode": mode,
            "outcome": outcome_name, "notable": outcome == Outcome::Findings,
            "census": { "coverage": coverage, "judged": judged, "in_scope": in_scope }
        });
        Evidence {
            report: AnalysisReport::parse(&raw.to_string(), &commit).unwrap(),
            platforms: PlatformCoverage::parse(
                "linux,windows",
                if complete { "linux,windows" } else { "linux" },
            )
            .unwrap(),
        }
    }

    #[test]
    fn cleanup_requires_both_a_clean_analysis_and_complete_collection() {
        for outcome in [
            Outcome::Findings,
            Outcome::Clean,
            Outcome::InsufficientBaseline,
            Outcome::NothingInScope,
            Outcome::Partial,
        ] {
            for complete in [false, true] {
                let result = evidence(AnalysisMode::History, outcome, complete)
                    .require_state(PublicationState::Clean);
                assert_eq!(result.is_ok(), outcome == Outcome::Clean && complete);
            }
        }
    }

    #[test]
    fn publication_projection_preserves_findings_and_distinguishes_incomplete_clean() {
        for (outcome, expected) in [
            (Outcome::Findings, PublicationState::Findings),
            (Outcome::Clean, PublicationState::Clean),
            (Outcome::Partial, PublicationState::NoData),
            (Outcome::InsufficientBaseline, PublicationState::NoData),
            (Outcome::NothingInScope, PublicationState::NoData),
        ] {
            let complete = evidence(AnalysisMode::Branch, outcome, true);
            assert_eq!(complete.publication_state(), expected);
            let incomplete = evidence(AnalysisMode::Branch, outcome, false);
            assert_eq!(
                incomplete.publication_state(),
                if outcome == Outcome::Findings {
                    PublicationState::Findings
                } else {
                    PublicationState::NoData
                }
            );
        }
    }

    #[test]
    fn platform_sets_are_trimmed_deduplicated_and_compared_by_identity() {
        let platforms = PlatformCoverage::parse(" windows,linux, linux ", "linux").unwrap();
        assert_eq!(platforms.completed(), ["linux"]);
        assert_eq!(platforms.missing(), ["windows"]);
        assert!(!platforms.is_complete());
        let platforms = PlatformCoverage::parse("linux,windows", " windows, linux ").unwrap();
        assert!(platforms.is_complete());
    }

    #[test]
    fn publication_modes_are_not_interchangeable() {
        let report = evidence(AnalysisMode::History, Outcome::Clean, true).report;
        report.require_mode(AnalysisMode::History).unwrap();
        let error = report.require_mode(AnalysisMode::Branch).unwrap_err();
        assert!(error.find_source::<WrongAnalysisMode>().is_some());
    }

    #[test]
    fn invalid_platform_evidence_is_rejected() {
        for (expected, completed) in [
            ("", "linux"),
            ("linux", ""),
            ("linux,", "linux"),
            ("linux", "windows"),
            ("linux", "linux,"),
            ("linux", "linux\ninjected"),
        ] {
            PlatformCoverage::parse(expected, completed).unwrap_err();
        }
    }

    #[test]
    fn malformed_inconsistent_dirty_and_wrong_commit_reports_are_rejected() {
        let sha: CommitSha = "a".repeat(40).parse().unwrap();
        let raw = serde_json::json!({
            "tip_commit": sha.as_str(), "tip_dirty": false, "mode": "history",
            "outcome": "clean", "notable": false,
            "census": { "coverage": "full", "judged": 1, "in_scope": 1 }
        });
        AnalysisReport::parse(&raw.to_string(), &sha).unwrap();
        for (field, value) in [
            ("outcome", serde_json::json!("failed")),
            ("notable", serde_json::json!(true)),
            ("tip_dirty", serde_json::json!(true)),
            ("tip_commit", serde_json::json!("b".repeat(40))),
            ("mode", serde_json::json!("other")),
            (
                "census",
                serde_json::json!({"coverage":"full","judged":0,"in_scope":0}),
            ),
        ] {
            let mut broken = raw.clone();
            *broken.get_mut(field).unwrap() = value;
            AnalysisReport::parse(&broken.to_string(), &sha).unwrap_err();
        }
        AnalysisReport::parse("{}", &sha).unwrap_err();
    }

    fn assert_invalid_census(outcome: &str, coverage: &str, judged: usize, in_scope: usize) {
        let sha: CommitSha = "a".repeat(40).parse().unwrap();
        let raw = serde_json::json!({
            "tip_commit": sha.as_str(), "tip_dirty": false, "mode": "history",
            "outcome": outcome, "notable": outcome == "findings",
            "census": { "coverage": coverage, "judged": judged, "in_scope": in_scope }
        });
        let error = AnalysisReport::parse(&raw.to_string(), &sha).unwrap_err();
        assert!(error.find_source::<InconsistentReport>().is_some());
    }

    #[test]
    fn empty_scope_cannot_contain_counted_series() {
        for coverage in ["no_series", "nothing_in_scope"] {
            assert_invalid_census("nothing_in_scope", coverage, 0, 1);
            assert_invalid_census("nothing_in_scope", coverage, 1, 0);
        }
    }

    #[test]
    fn nothing_judged_requires_unjudged_series_in_scope() {
        assert_invalid_census("insufficient_baseline", "nothing_judged", 0, 0);
        assert_invalid_census("insufficient_baseline", "nothing_judged", 1, 1);
    }

    #[test]
    fn partial_coverage_requires_both_judged_and_unjudged_series() {
        assert_invalid_census("partial", "partial", 0, 1);
        assert_invalid_census("partial", "partial", 1, 1);
        assert_invalid_census("partial", "partial", 2, 1);
    }

    #[test]
    fn findings_require_at_least_one_judged_series() {
        assert_invalid_census("findings", "no_series", 0, 0);
        assert_invalid_census("findings", "nothing_judged", 0, 1);
    }
}
