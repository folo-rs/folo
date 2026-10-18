use std::collections::BTreeSet;
use std::num::NonZero;

use ohno::AppError;
use serde::Serialize;

use crate::github::{GitHub, WorkflowJob};
use crate::operations::Context;
use crate::result::{Execution, platform_list};
use crate::workflow::args::BackfillJobsArgs;
use crate::workflow::files::write_new;
use crate::workflow::reconcile::{
    InvalidCollectionJobs, job_attempts, latest_job, terminal_execution,
};

/// Discovers normalized job executions before exporting completion-artifact ownership.
// Native dispatch tests cover the write-once boundary; job discovery and projection are
// independently covered through in-memory HTTP and semantic GitHub ports.
#[cfg_attr(test, mutants::skip)]
pub(crate) async fn backfill_jobs(
    github: &impl GitHub,
    context: &Context,
    args: BackfillJobsArgs,
) -> Result<(), AppError> {
    let expected = platform_list(&args.expected_platforms)?;
    let jobs = github
        .workflow_jobs(&context.repository, args.run_id)
        .await?;
    let output = backfill_job_outputs(context, args.run_id, &expected, &jobs)?;
    write_new(&args.output_file, &output)?;
    if context.verbose {
        eprintln!(
            "Selected latest normalized backfill executions for run {} and project {} across {:?}; missing jobs remain unavailable, and newer skipped or unsuccessful jobs supersede older successes.",
            args.run_id,
            context.instance.as_str(),
            expected,
        );
    }
    Ok(())
}

/// Exposes the exact selected job/attempt tuple without adopting completion files.
pub(crate) fn backfill_job_outputs(
    context: &Context,
    run_id: NonZero<u64>,
    expected: &BTreeSet<String>,
    jobs: &[WorkflowJob],
) -> Result<Vec<u8>, AppError> {
    // The action uses this protocol marker as the final reusable-workflow job-name segment.
    // Ref: docs/workflow-summary.md, "Backfill execution discovery".
    let prefix = format!("cbh-backfill:{}:", context.instance.as_str());
    let attempts = job_attempts(run_id, &prefix, expected, jobs)?;
    let mut platforms = Vec::new();
    for platform in expected {
        let Some(job) = latest_job(&attempts, platform) else {
            // A missing job has no trustworthy execution or attempt to attach a file to.
            continue;
        };
        let execution = terminal_execution(job)?;
        platforms.push(PlatformJob {
            platform,
            execution,
            conclusion: job
                .conclusion
                .as_deref()
                .expect("terminal_execution requires a known conclusion"),
            run_attempt: job.run_attempt,
            job_id: job.id,
        });
    }
    let output = BackfillJobs {
        version: JOBS_VERSION,
        repository: context.repository.to_string(),
        instance: context.instance.as_str(),
        run_id,
        platforms,
    };
    serde_json::to_vec(&output).map_err(|error| {
        InvalidCollectionJobs::caused_by("encoding backfill job evidence", error).into()
    })
}

// Independent of completion and measurement schemas; this envelope identifies workflow work.
const JOBS_VERSION: u32 = 1;

/// The action binds completion envelopes to this authenticated run/project selection.
#[derive(Debug, Serialize)]
struct BackfillJobs<'a> {
    version: u32,
    repository: String,
    instance: &'a str,
    run_id: NonZero<u64>,
    platforms: Vec<PlatformJob<'a>>,
}

/// An authoritative terminal execution, retaining its original attempt after reuse normalization.
#[derive(Debug, Serialize)]
struct PlatformJob<'a> {
    platform: &'a str,
    execution: Execution,
    conclusion: &'a str,
    run_attempt: NonZero<u64>,
    job_id: NonZero<u64>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn context() -> Context {
        Context {
            repository: "folo-rs/folo".parse().unwrap(),
            instance: "folo".parse().unwrap(),
            verbose: false,
        }
    }

    fn job(id: u64, platform: &str, attempt: u64, conclusion: &str) -> WorkflowJob {
        WorkflowJob {
            id: NonZero::new(id).unwrap(),
            run_id: NonZero::new(42).unwrap(),
            run_attempt: NonZero::new(attempt).unwrap(),
            name: format!("caller / cbh-backfill:folo:{platform}"),
            status: "completed".to_owned(),
            conclusion: Some(conclusion.to_owned()),
        }
    }

    fn project(jobs: &[WorkflowJob]) -> Result<Value, AppError> {
        let bytes = backfill_job_outputs(
            &context(),
            NonZero::new(42).unwrap(),
            &platform_list("linux,windows,macos").unwrap(),
            jobs,
        )?;
        Ok(serde_json::from_slice(&bytes).unwrap())
    }

    #[test]
    fn latest_skipped_job_supersedes_success_and_missing_platforms_stay_absent() {
        let mut unrelated = job(4, "linux", 3, "success");
        unrelated.name = "cbh-backfill:another-project:linux".to_owned();
        let jobs = [
            job(3, "linux", 2, "skipped"),
            job(2, "windows", 1, "success"),
            job(1, "linux", 1, "success"),
            unrelated,
        ];
        let result = project(&jobs).unwrap();
        assert_eq!(
            result,
            json!({
                "version":1, "repository":"folo-rs/folo", "instance":"folo", "run_id":42,
                "platforms":[
                    {"platform":"linux","execution":"skipped","conclusion":"skipped","run_attempt":2,"job_id":3},
                    {"platform":"windows","execution":"success","conclusion":"success","run_attempt":1,"job_id":2}
                ]
            })
        );
        assert_eq!(
            project(&jobs.into_iter().rev().collect::<Vec<_>>()).unwrap(),
            result
        );
    }

    #[test]
    fn terminal_non_success_states_never_restore_prior_success() {
        for (conclusion, execution) in [
            ("failure", "failure"),
            ("cancelled", "cancelled"),
            ("timed_out", "failure"),
            ("neutral", "failure"),
            ("action_required", "failure"),
        ] {
            let result = project(&[
                job(1, "linux", 1, "success"),
                job(2, "linux", 2, conclusion),
            ])
            .unwrap();
            let platform = result.get("platforms").unwrap().get(0).unwrap();
            assert_eq!(platform.get("execution").unwrap(), execution);
            assert_eq!(platform.get("conclusion").unwrap(), conclusion);
            assert_eq!(platform.get("run_attempt").unwrap(), 2);
        }
    }

    #[test]
    fn absent_jobs_emit_an_empty_selection_not_a_success() {
        let result = project(&[]).unwrap();
        assert_eq!(result.get("platforms").unwrap(), &json!([]));
    }

    #[test]
    fn unfinished_or_unknown_latest_jobs_reject_the_projection() {
        for (status, conclusion) in [
            ("in_progress", Some("success")),
            ("completed", None),
            ("completed", Some("unknown")),
        ] {
            let mut latest = job(2, "linux", 2, "success");
            latest.status = status.to_owned();
            latest.conclusion = conclusion.map(str::to_owned);
            let error = project(&[job(1, "linux", 1, "success"), latest]).unwrap_err();
            assert!(error.find_source::<InvalidCollectionJobs>().is_some());
        }
    }

    #[test]
    fn ambiguous_or_unexpected_job_identities_reject_the_projection() {
        let first = job(1, "linux", 1, "success");
        for second in [
            job(1, "windows", 1, "success"),
            job(2, "linux", 1, "success"),
            job(2, "unexpected", 1, "success"),
        ] {
            let error = project(&[first.clone(), second]).unwrap_err();
            assert!(error.find_source::<InvalidCollectionJobs>().is_some());
        }
        let mut foreign = first;
        foreign.run_id = NonZero::new(43).unwrap();
        let error = project(&[foreign]).unwrap_err();
        assert!(error.find_source::<InvalidCollectionJobs>().is_some());
    }
}
