use std::collections::BTreeMap;

use ohno::AppError;

use crate::github::{GitHub, WorkflowJob};
use crate::model::Instance;
use crate::operations::{Context, load_evidence};
use crate::workflow::args::{CollectionArgs, InspectArgs, MatrixArgs, PrepareArgs};
use crate::workflow::files::{
    append_outputs, canonical_directory, canonical_file, disjoint, fresh_directory, output_file,
    read_artifacts, read_file, read_results, write_new,
};
use crate::workflow::projection::{
    machine_key_files, matrix_outputs, preparation_diagnostics, preparation_outputs, report_outputs,
};
use crate::workflow::receipt::{
    InvalidMachineKey, Receipt, expected_platforms, machine_key, validate_platform,
};
use crate::workflow::reconcile::{Selection, reconcile};

// File-backed command adapters are covered natively; their transformations are pure unit targets.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn workflow_matrix(
    instance: &Instance,
    args: &MatrixArgs,
    verbose: bool,
) -> Result<(), AppError> {
    let outputs = matrix_outputs(&args.platforms, instance)?;
    append_outputs(&args.github_output, &outputs)?;
    if verbose {
        eprintln!(
            "Trimmed, deduplicated and sorted {:?} for instance {} so matrix jobs and expected collection coverage use the same platform identities.",
            args.platforms,
            instance.as_str()
        );
    }
    Ok(())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn collection_receipt(context: &Context, args: CollectionArgs) -> Result<(), AppError> {
    validate_platform(&args.platform)?;
    let key = read_file(&args.machine_key_file)?;
    let key = str::from_utf8(&key).map_err(InvalidMachineKey::caused_by)?;
    let receipt = Receipt {
        repository: context.repository.clone(),
        instance: context.instance.clone(),
        run_id: args.run_id,
        run_attempt: args.run_attempt,
        head: args.head,
        platform: args.platform,
        machine_key: machine_key(key.trim())?,
    };
    write_new(&args.file, &receipt.encode()?)?;
    if context.verbose {
        eprintln!(
            "Recorded platform {} for run {} attempt {} using measured machine key {}.",
            receipt.platform, receipt.run_id, receipt.run_attempt, receipt.machine_key
        );
    }
    Ok(())
}

#[cfg_attr(test, mutants::skip)]
pub(crate) async fn inspect_report(args: InspectArgs) -> Result<(), AppError> {
    let report = canonical_file(&args.evidence.report_file)?;
    let output = output_file(&args.github_output)?;
    disjoint(&report, &output)?;
    let evidence = load_evidence(args.evidence, &args.analyzed_sha).await?;
    append_outputs(&output, &report_outputs(&evidence))
}

#[cfg_attr(test, mutants::skip)]
pub(crate) async fn prepare_analysis(
    github: &impl GitHub,
    context: &Context,
    args: PrepareArgs,
) -> Result<(), AppError> {
    let jobs = github
        .workflow_jobs(&context.repository, args.run_id)
        .await?;
    prepare_from_jobs(context, &args, &jobs)
}

#[cfg_attr(test, mutants::skip)]
pub(crate) fn prepare_from_jobs(
    context: &Context,
    args: &PrepareArgs,
    jobs: &[WorkflowJob],
) -> Result<(), AppError> {
    let artifacts = read_artifacts(&args.receipts_dir)?;
    let selection = select_receipts(context, args, &artifacts.receipts, jobs)?;
    let objects = if args.local_results_dir.is_some() {
        read_results(selection.receipt_indices.iter().map(|index| {
            artifacts
                .roots
                .get(*index)
                .expect("selection indices address receipts with matching artifact roots")
                .clone()
        }))?
    } else {
        BTreeMap::new()
    };
    let source = canonical_directory(&args.receipts_dir)?;
    let keys = fresh_directory(&args.machine_key_dir)?;
    disjoint(&source, &keys)?;
    let results = args
        .local_results_dir
        .as_deref()
        .map(fresh_directory)
        .transpose()?;
    let output = output_file(&args.github_output)?;
    disjoint(&source, &output)?;
    disjoint(&keys, &output)?;
    if let Some(results) = &results {
        disjoint(&source, results)?;
        disjoint(&keys, results)?;
        disjoint(results, &output)?;
    }
    for (path, key) in machine_key_files(&selection, &artifacts.receipts) {
        write_new(&keys.join(path), key.as_bytes())?;
    }
    if let Some(results) = results {
        for (relative, bytes) in objects {
            write_new(&results.join(relative), &bytes)?;
        }
    }
    append_outputs(
        &output,
        &preparation_outputs(&selection, &artifacts.receipts),
    )
}

fn select_receipts(
    context: &Context,
    args: &PrepareArgs,
    receipts: &[Receipt],
    jobs: &[WorkflowJob],
) -> Result<Selection, AppError> {
    let expected = expected_platforms(&args.expected_platforms)?;
    let selection = reconcile(
        &context.repository,
        &context.instance,
        args.run_id,
        &args.head,
        &expected,
        jobs,
        receipts,
    )?;
    if context.verbose {
        for diagnostic in preparation_diagnostics(&selection, receipts, expected.len(), jobs.len())
        {
            eprintln!("{diagnostic}");
        }
    }
    Ok(selection)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use futures::executor::block_on;

    use super::*;
    use crate::github::WorkflowJob;
    use crate::github::fake::FakeGitHub;
    use crate::workflow::receipt::tests::receipt;

    #[test]
    fn orchestration_reads_job_evidence_through_the_semantic_port() {
        let receipt = receipt("linux", 1);
        let context = Context {
            repository: receipt.repository.clone(),
            instance: receipt.instance.clone(),
            verbose: false,
        };
        let github = FakeGitHub::new();
        github.set_jobs(vec![WorkflowJob {
            id: receipt.run_id,
            run_id: receipt.run_id,
            run_attempt: receipt.run_attempt,
            name: "cbh-collect:folo:linux".to_owned(),
            status: "completed".to_owned(),
            conclusion: Some("success".to_owned()),
        }]);
        let args = PrepareArgs {
            run_id: receipt.run_id,
            head: receipt.head.clone(),
            expected_platforms: "linux".to_owned(),
            receipts_dir: PathBuf::new(),
            machine_key_dir: PathBuf::new(),
            github_output: PathBuf::new(),
            local_results_dir: None,
        };
        let jobs = block_on(github.workflow_jobs(&context.repository, args.run_id)).unwrap();
        assert!(
            select_receipts(&context, &args, &[receipt], &jobs)
                .unwrap()
                .complete
        );
    }
}
