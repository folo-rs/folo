//! Downloadable worked reports and workflow cards for the first reader journey.
//!
//! Synthetic series feed production detection; production renderers own every verdict,
//! census and message. Invocation facts are illustrative, not claims about a real CI run.

use std::path::Path;

use cargo_bench_history_github::render_workflow_summary;
use cbh_render::{AnalysisOutcome, ReportFormat};
use serde_json::json;

use crate::assets::Asset;
use crate::figures::reporting::{Analysis, TIP_COMMIT, gallery_analysis, gallery_branch};

/// Registers full report downloads alongside the rendered excerpts used by the book.
#[must_use]
pub fn assets() -> Vec<Asset> {
    let mut assets = Vec::new();
    for outcome in AnalysisOutcome::ALL {
        let analysis = gallery_analysis(outcome);
        let name = outcome.as_str();
        assets.extend(bundle(name, &analysis));
        assets.push(Asset::new(
            format!("walkthrough-{name}-summary.md"),
            analysis.summary(),
        ));
    }
    let branch = gallery_branch();
    assets.extend(bundle("branch", &branch));
    assets.push(Asset::new("walkthrough-pr.md", branch.summary()));
    let report = branch.render(ReportFormat::Json);
    assets.push(Asset::new(
        "walkthrough-workflow.md",
        workflow_card(&report, TIP_COMMIT),
    ));
    // A different observed head establishes staleness only; it carries no age or distance.
    assets.push(Asset::new(
        "walkthrough-stale.md",
        workflow_card(&report, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    ));
    assets
}

/// Offers the same complete example in human-readable and machine-readable forms.
fn bundle(name: &str, analysis: &Analysis) -> Vec<Asset> {
    [
        ("txt", ReportFormat::Text),
        ("markdown", ReportFormat::Markdown),
        ("json", ReportFormat::Json),
    ]
    .into_iter()
    .map(|(extension, format)| {
        Asset::new(
            format!("walkthrough-{name}.{extension}"),
            analysis.render(format),
        )
    })
    .collect()
}

/// Feeds explicit example invocation facts and the genuine analysis JSON to the companion.
fn workflow_card(report: &str, observed_head: &str) -> String {
    let input = json!({
        "execution": "success",
        "publication": "disabled",
        // Reserved example domain prevents illustrations from linking to a real run.
        "run_url": "https://example.invalid/workflow",
        "artifact_url": "https://example.invalid/artifacts",
        "result": {
            "kind": "analysis",
            "report_file": "report.json",
            "analyzed_sha": TIP_COMMIT,
            "expected_platforms": "ubuntu-latest",
            "completed_platforms": "ubuntu-latest",
            "packages": ["textproc"],
            "observed_head": observed_head
        }
    });
    render_workflow_summary(input.to_string().as_bytes(), |path| {
        assert_eq!(path, Path::new("report.json"));
        Ok(report.as_bytes().to_vec())
    })
    .expect("the worked report and invocation facts satisfy the production card contract")
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;

    #[test]
    #[cfg_attr(
        miri,
        ignore = "the native gallery matrix runs the full worked statistical suites; small projection tests cover Miri"
    )]
    fn detector_outcomes_match_every_gallery_label() {
        for outcome in AnalysisOutcome::ALL {
            let report: Value =
                serde_json::from_str(&gallery_analysis(outcome).render(ReportFormat::Json))
                    .unwrap();
            assert_eq!(report.get("outcome").unwrap(), outcome.as_str());
            assert_eq!(
                report.get("notable").unwrap(),
                outcome == AnalysisOutcome::Findings
            );
            if outcome == AnalysisOutcome::Findings {
                let methods = report
                    .get("findings")
                    .unwrap()
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|finding| finding.get("method").unwrap().as_str().unwrap())
                    .collect::<Vec<_>>();
                assert!(methods.contains(&"change_point"));
                assert!(methods.contains(&"drift"));
            }
        }
        let report = gallery_branch().render(ReportFormat::Json);
        let report: Value = serde_json::from_str(&report).unwrap();
        assert_eq!(report.get("outcome").unwrap(), "findings");
        assert_eq!(report.get("mode").unwrap(), "branch");
        assert_eq!(report.get("ghosts_excluded").unwrap(), 0);
        let findings = report.get("findings").unwrap().as_array().unwrap();
        assert_eq!(findings.len(), 2);
        let directions = findings
            .iter()
            .map(|finding| finding.get("direction").unwrap().as_str().unwrap())
            .collect::<Vec<_>>();
        assert!(directions.contains(&"regression"));
        assert!(directions.contains(&"improvement"));
        for finding in findings {
            assert_eq!(finding.get("commit"), report.get("tip_commit"));
            assert_eq!(finding.get("engine").unwrap(), "callgrind");
            assert_eq!(finding.get("kind").unwrap(), "instruction_count");
        }
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "renders a full branch worked example; the companion tests small freshness projections under Miri"
    )]
    fn workflow_examples_share_evidence_but_not_freshness() {
        let report = gallery_branch().render(ReportFormat::Json);
        let current = workflow_card(&report, TIP_COMMIT);
        let stale = workflow_card(&report, "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        assert!(current.contains("Matches the observed head"));
        assert!(stale.contains("Out of date relative to the observed head"));
        assert!(current.contains("| Report disposition | findings |"));
        assert!(stale.contains("| Report disposition | findings |"));
    }
}
