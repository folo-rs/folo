use std::collections::BTreeSet;
use std::path::PathBuf;

use ohno::AppError;
use serde_json::json;

use crate::model::Instance;
use crate::result::{AnalysisMode, Evidence, Outcome};
use crate::workflow::receipt::{Receipt, expected_platforms};
use crate::workflow::reconcile::{Selection, collection_job_prefix};

pub(crate) fn matrix_outputs(platforms: &str, instance: &Instance) -> Result<String, AppError> {
    let platforms = expected_platforms(platforms)?;
    let expected = platforms
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(",");
    let matrix = json!({"platform": platforms});
    Ok(format!(
        "matrix={matrix}\nexpected-platforms={expected}\ninstance={}\ncollection-job-prefix={}\n",
        instance.as_str(),
        collection_job_prefix(instance)
    ))
}

pub(crate) fn preparation_outputs(selection: &Selection, receipts: &[Receipt]) -> String {
    let selected = selected_receipts(selection, receipts);
    let completed = selected
        .clone()
        .map(|receipt| receipt.platform.as_str())
        .collect::<Vec<_>>();
    let keys = selected
        .map(|receipt| receipt.machine_key.as_str())
        .collect::<BTreeSet<_>>();
    format!(
        "completed-platforms={}\nmachine-keys={}\ncomplete={}\n",
        completed.join(","),
        keys.into_iter().collect::<Vec<_>>().join(","),
        selection.complete
    )
}

pub(crate) fn machine_key_files(
    selection: &Selection,
    receipts: &[Receipt],
) -> Vec<(PathBuf, String)> {
    selected_receipts(selection, receipts)
        .map(|receipt| {
            (
                PathBuf::from(&receipt.platform).join("machine-key.txt"),
                format!("{}\n", receipt.machine_key),
            )
        })
        .collect()
}

fn selected_receipts<'a>(
    selection: &'a Selection,
    receipts: &'a [Receipt],
) -> impl Iterator<Item = &'a Receipt> + Clone {
    selection.receipt_indices.iter().map(|index| {
        receipts
            .get(*index)
            .expect("selection indices come from these receipts")
    })
}

pub(crate) fn report_outputs(evidence: &Evidence) -> String {
    let outcome = evidence.report.outcome.as_str();
    // Use the publication gates themselves: their validated parser owns census semantics.
    let notable = evidence.report.outcome == Outcome::Findings;
    let can_clear = evidence.report.mode == AnalysisMode::History && evidence.is_all_clear();
    format!("outcome={outcome}\nnotable={notable}\ncan-clear={can_clear}\n")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::slice;

    use super::*;
    use crate::github::WorkflowJob;
    use crate::result::InvalidPlatformList;
    use crate::result::tests::evidence;
    use crate::workflow::receipt::InvalidCollectionPlatform;
    use crate::workflow::receipt::tests::receipt;
    use crate::workflow::reconcile::reconcile;

    #[test]
    fn matrix_outputs_share_normalized_platforms_and_instance() {
        let instance = "portable".parse().unwrap();
        assert_eq!(
            matrix_outputs(" windows,linux,windows ", &instance).unwrap(),
            concat!(
                "matrix={\"platform\":[\"linux\",\"windows\"]}\n",
                "expected-platforms=linux,windows\n",
                "instance=portable\n",
                "collection-job-prefix=cbh-collect:portable\n"
            )
        );
    }

    #[test]
    fn matrix_outputs_reuse_preparation_platform_validation() {
        let instance = "portable".parse().unwrap();
        let error = matrix_outputs("linux,", &instance).unwrap_err();
        assert!(error.find_source::<InvalidPlatformList>().is_some());
        let error = matrix_outputs("..", &instance).unwrap_err();
        assert!(error.find_source::<InvalidCollectionPlatform>().is_some());
    }

    #[test]
    fn matrix_generated_collection_names_are_recognized_by_preparation() {
        let mut receipt = receipt("linux", 1);
        receipt.instance = "portable".parse().unwrap();
        let outputs = matrix_outputs("linux", &receipt.instance).unwrap();
        let prefix = outputs
            .lines()
            .find_map(|line| line.strip_prefix("collection-job-prefix="))
            .unwrap();
        let job = WorkflowJob {
            id: receipt.run_id,
            run_id: receipt.run_id,
            run_attempt: receipt.run_attempt,
            name: format!("reusable / {prefix}:linux"),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
        };
        let selection = reconcile(
            &receipt.repository,
            &receipt.instance,
            receipt.run_id,
            &receipt.head,
            &expected_platforms("linux").unwrap(),
            &[job],
            slice::from_ref(&receipt),
        )
        .unwrap();
        assert!(selection.complete);
        assert_eq!(selection.receipt_indices, [0]);
    }

    #[test]
    fn keys_are_deduplicated_but_platforms_remain_independent() {
        let receipts = [receipt("linux", 1), receipt("windows", 2)];
        assert_eq!(
            preparation_outputs(
                &Selection {
                    receipt_indices: vec![0, 1],
                    complete: true
                },
                &receipts,
            ),
            "completed-platforms=linux,windows\nmachine-keys=0123456789abcdef\ncomplete=true\n"
        );
    }

    #[test]
    fn machine_key_tree_contains_only_selected_actual_keys() {
        let mut windows = receipt("windows", 2);
        windows.machine_key = "fedcba9876543210".to_owned();
        let receipts = [receipt("linux", 1), windows];
        let selection = Selection {
            receipt_indices: vec![1],
            complete: false,
        };
        assert_eq!(
            machine_key_files(&selection, &receipts),
            vec![(
                PathBuf::from("windows").join("machine-key.txt"),
                "fedcba9876543210\n".to_owned()
            )]
        );
        assert_eq!(
            preparation_outputs(&selection, &receipts),
            "completed-platforms=windows\nmachine-keys=fedcba9876543210\ncomplete=false\n"
        );
    }

    #[test]
    fn output_projection_uses_report_and_platform_evidence() {
        for (outcome, wire) in [
            (Outcome::Findings, "findings"),
            (Outcome::Clean, "clean"),
            (Outcome::Partial, "partial"),
            (Outcome::InsufficientBaseline, "insufficient_baseline"),
            (Outcome::NothingInScope, "nothing_in_scope"),
        ] {
            let evidence = evidence(AnalysisMode::History, outcome, true);
            assert_eq!(
                report_outputs(&evidence),
                format!(
                    "outcome={wire}\nnotable={}\ncan-clear={}\n",
                    outcome == Outcome::Findings,
                    outcome == Outcome::Clean
                )
            );
        }
    }

    #[test]
    fn branch_and_incomplete_collection_never_clear_issues() {
        for evidence in [
            evidence(AnalysisMode::Branch, Outcome::Clean, true),
            evidence(AnalysisMode::History, Outcome::Clean, false),
        ] {
            assert_eq!(
                report_outputs(&evidence),
                "outcome=clean\nnotable=false\ncan-clear=false\n"
            );
        }
    }
}
