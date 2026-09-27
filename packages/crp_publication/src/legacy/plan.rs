use std::collections::{BTreeMap, BTreeSet};

use ohno::AppError;

use crate::PublicationOutput;
use crate::legacy::model::{Batch, Plan, runner_label, timeout_minutes};
use crate::publication::binaries::model::{InvalidPlan, identifier};
use crate::publication::binaries::{Asset, Binary};

pub(crate) fn plan(
    input: Plan,
    mut assets: impl FnMut(&Binary) -> Result<Vec<Asset>, AppError>,
    diagnostics: &PublicationOutput,
) -> Result<Vec<Batch>, AppError> {
    // Validate every local request before remote reads: malformed input must not depend on
    // network availability or consume query capacity for an earlier, otherwise valid item.
    let mut targets = BTreeMap::new();
    for target in input.targets {
        if !identifier(&target.triple)
            || !runner_label(&target.runner)
            || targets.insert(target.triple, target.runner).is_some()
        {
            return Err(InvalidPlan::new("Invalid or duplicate release target".to_owned()).into());
        }
    }
    if targets.is_empty() {
        return Err(InvalidPlan::new("No release targets were supplied".to_owned()).into());
    }
    let mut identities = BTreeSet::new();
    for request in &input.binaries {
        request.binary.validate()?;
        if !identities.insert((&request.binary.name, &request.binary.version)) {
            return Err(InvalidPlan::new(format!(
                "Duplicate release request: {}",
                request.binary.tag
            ))
            .into());
        }
        for triple in &request.release_targets {
            if !targets.contains_key(triple) {
                return Err(InvalidPlan::new(format!(
                    "{} declares unsupported target {triple}; update the supported target selection or the package's release-plan.release-targets metadata",
                    request.binary.name
                )).into());
            }
        }
    }
    let mut batches = BTreeMap::<String, Batch>::new();
    for request in input.binaries {
        let assets = assets(&request.binary)?;
        for (triple, runner) in &targets {
            if !request.release_targets.is_empty() && !request.release_targets.contains(triple) {
                diagnostics.line(format_args!(
                    "{} on {triple}: excluded by package release-targets {:?}.",
                    request.binary.tag, request.release_targets
                ));
                continue;
            }
            if request.binary.complete(triple, &assets) {
                diagnostics.line(format_args!(
                    "{} on {triple}: both release assets are uploaded; skipping.",
                    request.binary.tag
                ));
                continue;
            }
            diagnostics.line(format_args!(
                "{} on {triple}: archive/checksum pair is incomplete; scheduling {runner}.",
                request.binary.tag
            ));
            batches
                .entry(triple.clone())
                .or_insert_with(|| Batch {
                    triple: triple.clone(),
                    runner: runner.clone(),
                    timeout_minutes: 0,
                    binaries: Vec::new(),
                })
                .binaries
                .push(request.binary.clone());
        }
    }
    for batch in batches.values_mut() {
        batch.binaries.sort_by(|a, b| {
            (&a.source_sha, &a.name, &a.version).cmp(&(&b.source_sha, &b.name, &b.version))
        });
        batch.timeout_minutes = timeout_minutes(batch.binaries.len());
        batch.validate()?;
    }
    Ok(batches.into_values().collect())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::indexing_slicing,
    reason = "Test fixtures specify every indexed item"
)]
mod tests {
    use std::sync::Arc;

    use crp_diag::Discard;

    use super::*;
    use crate::legacy::model::{Request, Target};
    use crate::publication::binaries::model::tests::binary;

    fn plan(
        input: Plan,
        assets: impl FnMut(&Binary) -> Result<Vec<Asset>, AppError>,
    ) -> Result<Vec<Batch>, AppError> {
        super::plan(
            input,
            assets,
            &PublicationOutput::new("1.0.0", false, Arc::new(Discard)),
        )
    }

    fn input() -> Plan {
        Plan {
            targets: (0..5)
                .map(|index| Target {
                    triple: format!("target-{index}"),
                    runner: format!("runner-{index}"),
                })
                .collect(),
            binaries: ["alpha", "beta", "gamma"]
                .map(|name| Request {
                    binary: binary(name),
                    release_targets: vec![],
                })
                .into(),
        }
    }

    #[test]
    fn conserves_fifteen_pairs_in_five_batches() {
        let batches = plan(input(), |_| Ok(vec![])).unwrap();
        assert_eq!(batches.len(), 5);
        for (index, batch) in batches.iter().enumerate() {
            assert_eq!(batch.triple, format!("target-{index}"));
            assert_eq!(batch.runner, format!("runner-{index}"));
            assert_eq!(batch.binaries.len(), 3);
            assert_eq!(batch.timeout_minutes, 270);
            assert_eq!(
                batch
                    .binaries
                    .iter()
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>(),
                ["alpha", "beta", "gamma"]
            );
        }
    }

    #[test]
    fn runner_labels_with_platform_versions_are_valid() {
        let mut input = input();
        input.targets[0].runner = "ubuntu-24.04-arm".into();
        let batches = plan(input, |_| Ok(vec![])).unwrap();
        assert_eq!(batches[0].runner, "ubuntu-24.04-arm");
    }

    #[test]
    fn restrictions_and_complete_assets_only_remove_their_own_pairs() {
        let mut input = input();
        input.binaries[0].release_targets = vec!["target-0".into()];
        input.binaries[1].binary.source_sha = "b".repeat(40);
        let batches = plan(input, |binary| {
            Ok(vec![
                Asset {
                    name: format!("{}.zip", binary.archive_base("target-1")),
                    state: "uploaded".into(),
                },
                Asset {
                    name: format!("{}.sha256", binary.archive_base("target-1")),
                    state: "uploaded".into(),
                },
            ])
        })
        .unwrap();
        assert_eq!(batches.len(), 4);
        assert_eq!(batches[0].binaries.len(), 3);
        assert_eq!(batches[1].binaries.len(), 2);
        assert_eq!(
            batches[0].binaries.last().unwrap().source_sha,
            "b".repeat(40)
        );
    }

    #[test]
    fn invalid_input_is_rejected_before_queries() {
        let mut empty = input();
        empty.targets.clear();
        plan(empty, |_| panic!()).unwrap_err();
        let mut duplicate = input();
        duplicate.binaries.push(Request {
            binary: binary("alpha"),
            release_targets: vec![],
        });
        plan(duplicate, |_| panic!()).unwrap_err();
        let mut input = input();
        input.binaries[0].release_targets.push("unknown".into());
        plan(input, |_| panic!()).unwrap_err();
        let mut input = self::input();
        input.targets.push(Target {
            triple: "target-0".into(),
            runner: "different".into(),
        });
        plan(input, |_| panic!()).unwrap_err();
    }

    #[test]
    fn empty_singleton_and_failed_queries_are_distinct() {
        let mut input = input();
        input.binaries.clear();
        assert_eq!(
            serde_json::to_string(&plan(input, |_| panic!()).unwrap()).unwrap(),
            "[]"
        );
        let mut input = self::input();
        input.binaries.truncate(1);
        input.targets.truncate(1);
        let result = plan(input, |_| Ok(vec![])).unwrap();
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.is_array());
        assert!(json[0]["binaries"].is_array());
        plan(self::input(), |_| {
            Err(InvalidPlan::new("query failed".to_owned()).into())
        })
        .unwrap_err();
    }
}
