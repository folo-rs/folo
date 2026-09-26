//! Explicit registry prerequisites for the local increment workflow, separate from offline checks.

use std::path::Path;

use ohno::AppError;
use serde::Deserialize;

use crate::inspect_plan::run_inspect_plan;
use crate::metadata::load_tracked_work_tree;
use crate::publication::registry::RegistryClient;
use crate::verbose::Verbose;

#[derive(Deserialize)]
struct PlanTargets {
    publication_targets: Vec<String>,
}

pub(crate) fn check(
    manifest: &Path,
    plan: Option<&Path>,
    verbose: Verbose,
) -> Result<(bool, String), AppError> {
    let targets = if let Some(plan) = plan {
        serde_json::from_str::<PlanTargets>(&run_inspect_plan(plan, true, manifest, verbose)?)?
            .publication_targets
    } else {
        load_tracked_work_tree(manifest)?
            .0
            .packages
            .into_iter()
            .map(|package| package.manifest.name)
            .collect()
    };
    let client = RegistryClient::new()?;
    let mut missing = Vec::new();
    let mut unknown = Vec::new();
    for name in targets {
        match client.exists(&name) {
            Ok(true)=>verbose.note(||format!("{name} has an established registry identity; no first-publication handoff is required.")),
            Ok(false)=>missing.push(name),
            Err(error)=>{eprintln!("{name}: {error}");unknown.push(name);}
        }
    }
    Ok(conclusion(plan.is_some(), &missing, &unknown))
}

fn conclusion(required: bool, missing: &[String], unknown: &[String]) -> (bool, String) {
    let passed = missing.is_empty() && unknown.is_empty();
    let message = if passed {
        "Every selected publishable package is established on crates.io.".to_owned()
    } else {
        format!(
            "First-publication prerequisites: missing [{}]; unavailable registry evidence [{}]. \
            A maintainer must bootstrap new packages and configure Trusted Publishing; this command publishes nothing.",
            missing.join(", "),
            unknown.join(", ")
        )
    };
    // Workspace discovery is advisory; the exact resolved-plan gate is fail-closed.
    (passed || !required, message)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn resolved_plan_requires_positive_publication_evidence_but_discovery_is_advisory() {
        assert!(conclusion(true, &[], &[]).0);
        for (missing, unknown) in [
            (vec!["new".to_owned()], vec![]),
            (vec![], vec!["unavailable".to_owned()]),
        ] {
            assert!(!conclusion(true, &missing, &unknown).0);
            let advisory = conclusion(false, &missing, &unknown);
            assert!(advisory.0);
            assert!(advisory.1.contains("First-publication prerequisites"));
        }
    }
}
