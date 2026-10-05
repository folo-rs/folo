use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crp_diag::Verbose;
use crp_publication::PublicationOutput;
use crp_publication::publication::binaries::publish::publish as publish_binaries;
use crp_publication::publication::context::{CONTEXT_SCHEMA_VERSION, release_context};
use crp_publication::publication::credentials::provide;
use crp_publication::publication::github::publish as publish_github;
use crp_publication::publication::identity::check_publishing_identity;
use crp_publication::publication::packages::check_publication;
use crp_publication::publication::preflight::check as check_published;
use crp_publication::publication::prepare::prepare as prepare_publication;
use crp_publication::publication::registry::publish as publish_registry;
use crp_publication::publication::report::report as report_publication;
use crp_versioning::analysis_order::run_analysis_order;
use crp_versioning::apply::run_apply;
use crp_versioning::inspect_plan::run_inspect_plan;
use crp_versioning::plan::SCHEMA_VERSION;
use crp_versioning::preview::{run_prepare_with_cache, run_preview_with_options};
use crp_versioning::propose::{DECISION_SCHEMA_VERSION, run_propose};
use crp_versioning::report::run_report_with_cache;
use crp_versioning::resolved::run_verify_preview;
use crp_versioning::semver_targets::run_semver_targets;
use crp_versioning::{CheckFormat, CheckRequest, check_with_cache};
use crp_workspace::cache::{Cache, CacheOptions};
use ohno::AppError;

use crate::compatibility::{
    COMPATIBILITY_SCHEMA_VERSION, check_with_target as check_compatibility,
};

/// Input parameters for [`run`].
#[derive(Debug)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Application code and maintainer tests exhaustively match internal command inputs"
)]
pub enum RunInput {
    /// Applies an explicit storage policy to a classification command.
    Cached {
        command: Box<Self>,
        cache: CacheOptions,
    },
    /// Report the installed executable and its artifact contracts without a workspace.
    Version,
    /// Check crates.io publication identities without uploading or changing source.
    CheckPublished {
        manifest_path: PathBuf,
        plan: Option<PathBuf>,
        verbose: bool,
    },
    /// Collect external API compatibility evidence from captured or fresh source.
    CheckCompatibility {
        manifest_path: PathBuf,
        prepared: Option<PathBuf>,
        plan: Option<PathBuf>,
        release_history: Option<String>,
        merge_target: Option<String>,
        output: PathBuf,
        deny_findings: bool,
        verbose: bool,
    },
    /// Report completeness from current workflow job results and retained receipts.
    ///
    /// Incomplete publication creates or updates the run-qualified GitHub failure issue
    /// unless `no_issue` suppresses that remote write.
    PublicationReport {
        repository: String,
        publication: Option<PathBuf>,
        outcomes: PathBuf,
        jobs: PathBuf,
        output: PathBuf,
        no_issue: bool,
    },
    /// Resolve configured release history and workflow scope without preparing publication.
    ReleaseContext {
        manifest_path: PathBuf,
        config: Option<PathBuf>,
        release_history: Option<String>,
        merge_target: Option<String>,
        verbose: bool,
    },
    /// Verify the GitHub caller's Trusted Publishing identity without uploading.
    CheckPublishingIdentity { verbose: bool },
    /// Build and publish one frozen platform batch.
    PublishBinaries {
        publication: PathBuf,
        batch: PathBuf,
        manifest_path: PathBuf,
        output: PathBuf,
        artifacts: PathBuf,
        no_upload: bool,
    },
    /// Reconcile GitHub tags/releases and emit platform batches.
    PublishGithub {
        publication: PathBuf,
        manifest_path: PathBuf,
        output: PathBuf,
        batches: PathBuf,
        dry_run: bool,
        verbose: bool,
    },
    /// Reconcile exact crate versions and publish only those missing from crates.io.
    PublishRegistry {
        /// Immutable publication manifest.
        publication: PathBuf,
        /// Cargo manifest in the original source checkout.
        manifest_path: PathBuf,
        /// Structured phase outcome destination.
        output: PathBuf,
        /// Observe and describe missing versions without credentials or uploads.
        dry_run: bool,
        /// Explain reconciliation inputs and decisions.
        verbose: bool,
    },
    /// Serve Cargo's internal per-upload credential protocol.
    CredentialProvider,
    /// Capture immutable publication intent from a clean merged source snapshot.
    PreparePublish {
        /// Source checkout's Cargo manifest.
        manifest_path: PathBuf,
        /// Workspace-relative publication configuration; omitted selects the conventional file.
        config: Option<PathBuf>,
        /// Immutable source commit that must match the checkout.
        source: String,
        /// Publication manifest destination.
        output: PathBuf,
        /// Explain captured release inputs.
        verbose: bool,
    },
    /// Inspect validated expanded-plan facts for external tooling.
    InspectPlan {
        /// Expanded plan artifact.
        plan: PathBuf,
        /// Require a captured preview valid for application.
        require_resolved: bool,
        /// Workspace supplying tracked membership and publication eligibility.
        manifest_path: PathBuf,
        /// Print explanatory validation decisions.
        verbose: bool,
    },
    /// Order report packages for semantic assessment.
    AnalysisOrder {
        /// Report file or its containing directory.
        report: PathBuf,
        /// Print explanatory ordering decisions.
        verbose: bool,
    },
    /// Select consumer-contract packages for compatibility assessment.
    SemverTargets {
        /// Report file or its containing directory.
        report: PathBuf,
        /// Print explanatory target decisions.
        verbose: bool,
    },
    /// Complete caller-supplied semantic decisions using captured report evidence.
    Propose {
        /// Report file or its containing directory.
        report: PathBuf,
        /// Caller-supplied change decisions.
        decisions: PathBuf,
        /// Destination for the proposed plan.
        out: PathBuf,
        /// Print explanatory version-resolution decisions.
        verbose: bool,
    },
    /// Capture prepared evidence after refreshing the live lockfile offline.
    Prepare {
        /// Directory receiving report evidence and prepared.json.
        output: PathBuf,
        /// Actual release history; defaults to the remote default branch.
        release_history: Option<String>,
        /// Optional anticipated parent release, using its final content and versions.
        merge_target: Option<String>,
        /// Workspace manifest to prepare.
        manifest_path: PathBuf,
        /// Print explanatory resolver decisions.
        verbose: bool,
    },
    /// Resolve a proposed plan to a complete, captured state for application.
    Preview {
        /// Semantic release proposal.
        plan: PathBuf,
        /// Prepared artifact whose report supplied semantic-assessment evidence.
        prepared: PathBuf,
        /// Directory receiving the final report, plan, and compatibility workspace.
        output: PathBuf,
        /// Workspace manifest whose inputs must match preparation.
        manifest_path: PathBuf,
        /// Print explanatory expansion and resolver decisions.
        verbose: bool,
    },
    /// Check that compatibility evidence uses the captured final workspace unchanged.
    VerifyPreview {
        /// Resolved plan whose captured state must match.
        plan: PathBuf,
        /// Required retained candidate manifest; the original workspace is not accepted.
        manifest_path: PathBuf,
        /// Print explanatory verification notes.
        verbose: bool,
    },
    /// `report` — write `report.json` and per-package diffs.
    Report {
        /// Directory that receives `report.json` and `diffs/`.
        out_dir: PathBuf,
        /// Actual release-history commit whose first-parent line supplies anchors.
        ///
        /// `None` defers to the default branch of the `origin` remote.
        release_history: Option<String>,
        /// Optional anticipated parent release, using its final content and versions.
        merge_target: Option<String>,
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// Validate workspace version support and optional publication configuration.
    Check {
        /// Actual release-history commit whose first-parent line supplies anchors.
        ///
        /// `None` defers to the default branch of the `origin` remote.
        release_history: Option<String>,
        /// Optional anticipated parent release, using its final content and versions.
        merge_target: Option<String>,
        /// Workspace manifest to classify. Used verbatim.
        manifest_path: PathBuf,
        /// How to render diagnostics.
        format: CheckFormat,
        /// When set, warn on divergence from `cargo package --list` without failing.
        verify_packaging: bool,
        /// Optional publication configuration, relative to the selected workspace.
        config: Option<PathBuf>,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
    /// Install the exact manifest and lockfile edits recorded by preview.
    Apply {
        /// Path to the plan JSON file.
        plan: PathBuf,
        /// When set, validate and describe planned writes without changing files.
        dry_run: bool,
        /// Workspace manifest to edit. Used verbatim.
        manifest_path: PathBuf,
        /// When set, print explanatory decision notes to stderr.
        verbose: bool,
    },
}

/// The successful outcome of a run.
#[derive(Clone, Debug, Eq, PartialEq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "Application code and maintainer tests exhaustively match internal command outcomes"
)]
pub enum RunOutcome {
    /// Exchanged GitHub OIDC identity and revoked the resulting crates.io credential.
    IdentityCheck { message: String },
    /// A publication-related operation wrote its outcome or final Markdown report.
    Publication {
        /// Whether the requested operation succeeded; dry runs do not establish delivery.
        passed: bool,
        /// Human-readable disposition and output artifact location.
        message: String,
    },
    /// A JSON-producing query completed.
    ArtifactQuery {
        /// JSON document for stdout.
        ///
        /// Empty only when the credential provider has already written its protocol stream.
        message: String,
    },
    /// A proposed release plan was written.
    Propose {
        /// Human-readable summary.
        message: String,
    },
    /// Preparation completed and wrote prepared evidence.
    Prepare {
        /// Human-readable summary.
        message: String,
    },
    /// Preview completed and wrote the resolved plan.
    Preview {
        /// Human-readable summary.
        message: String,
    },
    /// The retained compatibility workspace matches the resolved plan.
    VerifyPreview {
        /// Human-readable summary.
        message: String,
    },
    /// `report` finished and wrote its artifacts.
    Report {
        /// Human-readable summary for stdout. Empty when there is nothing to say.
        message: String,
    },
    /// A requested check finished with its process-level verdict.
    Check {
        /// Whether the requested check passed under its selected gating mode.
        passed: bool,
        /// Rendered gating diagnostics or a success summary.
        message: String,
        /// Non-gating advisory lines for stderr.
        warnings: String,
    },
    /// `apply` finished (including `--dry-run`).
    Apply {
        /// Human-readable summary for stdout.
        message: String,
    },
}

/// Executes one requested operation and reports its outcome.
///
/// Selects the command named by `input` and returns its summary or the check
/// verdict and diagnostics.
///
/// # Errors
///
/// Returns an application error when the requested operation cannot be
/// completed. A failing check is a [`RunOutcome::Check`] with
/// `passed: false`, not an error.
pub fn run(input: &RunInput) -> Result<RunOutcome, AppError> {
    run_with_cache(input, &CacheOptions::Default)
}

fn run_with_cache(input: &RunInput, cache_options: &CacheOptions) -> Result<RunOutcome, AppError> {
    match input {
        RunInput::Cached { command, cache } => run_with_cache(command, cache),
        RunInput::Version => Ok(RunOutcome::ArtifactQuery {
            message: serde_json::to_string(&serde_json::json!({
                "tool_version": env!("CARGO_PKG_VERSION"),
                "schemas": {
                    "plan": SCHEMA_VERSION,
                    "report": SCHEMA_VERSION,
                    "prepared": SCHEMA_VERSION,
                    "decisions": DECISION_SCHEMA_VERSION,
                    "compatibility": COMPATIBILITY_SCHEMA_VERSION,
                    "release_context": CONTEXT_SCHEMA_VERSION
                }
            }))?,
        }),
        RunInput::CheckPublished {
            manifest_path,
            plan,
            verbose,
        } => {
            let (passed, message) = check_published(
                manifest_path,
                plan.as_deref(),
                &publication_output(*verbose),
            )?;
            Ok(RunOutcome::Check {
                passed,
                message,
                warnings: String::new(),
            })
        }
        RunInput::CheckCompatibility {
            manifest_path,
            prepared,
            plan,
            release_history,
            merge_target,
            output,
            deny_findings,
            verbose,
        } => {
            let (passed, message) = check_compatibility(
                manifest_path,
                prepared.as_deref(),
                plan.as_deref(),
                release_history.as_deref(),
                merge_target.as_deref(),
                output,
                *deny_findings,
                *verbose,
                cache_options,
            )?;
            Ok(RunOutcome::Check {
                passed,
                message,
                warnings: String::new(),
            })
        }
        RunInput::PublicationReport {
            repository,
            publication,
            outcomes,
            jobs,
            output,
            no_issue,
        } => {
            let (passed, message) = report_publication(
                repository,
                publication.as_deref(),
                outcomes,
                jobs,
                output,
                *no_issue,
                &publication_output(false),
            )?;
            Ok(RunOutcome::Publication { passed, message })
        }
        RunInput::ReleaseContext {
            manifest_path,
            config,
            release_history,
            merge_target,
            verbose,
        } => {
            let message = release_context(
                manifest_path,
                config.as_deref(),
                release_history.as_deref(),
                merge_target.as_deref(),
                Verbose::new(*verbose, &crp_diag::Stderr),
            )?;
            Ok(RunOutcome::ArtifactQuery { message })
        }
        RunInput::CheckPublishingIdentity { verbose } => {
            let message = check_publishing_identity(&publication_output(*verbose))?;
            Ok(RunOutcome::IdentityCheck { message })
        }
        RunInput::PublishBinaries {
            publication,
            batch,
            manifest_path,
            output,
            artifacts,
            no_upload,
        } => {
            let (passed, message) = publish_binaries(
                publication,
                batch,
                manifest_path,
                output,
                artifacts,
                *no_upload,
                &publication_output(false),
            )?;
            Ok(RunOutcome::Publication { passed, message })
        }
        RunInput::PublishGithub {
            publication,
            manifest_path,
            output,
            batches,
            dry_run,
            verbose,
        } => {
            let (passed, message) = publish_github(
                publication,
                manifest_path,
                output,
                batches,
                *dry_run,
                &publication_output(*verbose),
            )?;
            Ok(RunOutcome::Publication { passed, message })
        }
        RunInput::PublishRegistry {
            publication,
            manifest_path,
            output,
            dry_run,
            verbose,
        } => {
            let (passed, message) = publish_registry(
                publication,
                manifest_path,
                output,
                *dry_run,
                &publication_output(*verbose),
            )?;
            Ok(RunOutcome::Publication { passed, message })
        }
        RunInput::CredentialProvider => {
            provide(
                &mut io::stdin().lock(),
                &mut io::stdout().lock(),
                &publication_output(false),
            )?;
            // The provider owns stdout's line-oriented protocol. An empty summary prevents
            // the ordinary entry-point dispatcher from appending any unrelated bytes.
            Ok(RunOutcome::ArtifactQuery {
                message: String::new(),
            })
        }
        RunInput::PreparePublish {
            manifest_path,
            config,
            source,
            output,
            verbose,
        } => {
            let message = prepare_publication(
                manifest_path,
                config.as_deref(),
                source,
                output,
                &publication_output(*verbose),
            )?;
            Ok(RunOutcome::Prepare { message })
        }
        RunInput::InspectPlan {
            plan,
            require_resolved,
            manifest_path,
            verbose,
        } => {
            let message = run_inspect_plan(
                plan,
                *require_resolved,
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
            )?;
            Ok(RunOutcome::ArtifactQuery { message })
        }
        RunInput::AnalysisOrder { report, verbose } => {
            let message = run_analysis_order(report, Verbose::new(*verbose, &crp_diag::Stderr))?;
            Ok(RunOutcome::ArtifactQuery { message })
        }
        RunInput::SemverTargets { report, verbose } => {
            let message = run_semver_targets(report, Verbose::new(*verbose, &crp_diag::Stderr))?;
            Ok(RunOutcome::ArtifactQuery { message })
        }
        RunInput::Propose {
            report,
            decisions,
            out,
            verbose,
        } => {
            let message = run_propose(
                report,
                decisions,
                out,
                Verbose::new(*verbose, &crp_diag::Stderr),
            )?;
            Ok(RunOutcome::Propose { message })
        }
        RunInput::VerifyPreview {
            plan,
            manifest_path,
            verbose,
        } => {
            let message = run_verify_preview(
                plan,
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
            )?;
            Ok(RunOutcome::VerifyPreview { message })
        }
        RunInput::Prepare {
            output,
            release_history,
            merge_target,
            manifest_path,
            verbose,
        } => {
            let message = run_prepare_with_cache(
                output,
                release_history.as_deref(),
                merge_target.as_deref(),
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
                Cache::resolve(
                    manifest_path,
                    cache_options,
                    Verbose::new(*verbose, &crp_diag::Stderr),
                )?,
            )?;
            Ok(RunOutcome::Prepare { message })
        }
        RunInput::Preview {
            plan,
            prepared,
            output,
            manifest_path,
            verbose,
        } => {
            let message = run_preview_with_options(
                plan,
                prepared,
                output,
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
                cache_options,
            )?;
            Ok(RunOutcome::Preview { message })
        }
        RunInput::Report {
            out_dir,
            release_history,
            merge_target,
            manifest_path,
            verbose,
        } => {
            let message = run_report_with_cache(
                out_dir,
                release_history.as_deref(),
                merge_target.as_deref(),
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
                Cache::resolve(
                    manifest_path,
                    cache_options,
                    Verbose::new(*verbose, &crp_diag::Stderr),
                )?,
            )?;
            Ok(RunOutcome::Report { message })
        }
        RunInput::Check {
            release_history,
            merge_target,
            manifest_path,
            format,
            verify_packaging,
            config,
            verbose,
        } => {
            if let Some(config) = config {
                check_publication(
                    manifest_path,
                    config,
                    Verbose::new(*verbose, &crp_diag::Stderr),
                )?;
            }
            let outcome = check_with_cache(
                &CheckRequest {
                    release_history: release_history.as_deref(),
                    manifest_path,
                    format: *format,
                    verify_packaging: *verify_packaging,
                },
                merge_target.as_deref(),
                Verbose::new(*verbose, &crp_diag::Stderr),
                Cache::resolve(
                    manifest_path,
                    cache_options,
                    Verbose::new(*verbose, &crp_diag::Stderr),
                )?,
            )?;
            Ok(RunOutcome::Check {
                passed: outcome.passed,
                message: outcome.message,
                warnings: outcome.warnings,
            })
        }
        RunInput::Apply {
            plan,
            dry_run,
            manifest_path,
            verbose,
        } => {
            let message = run_apply(
                plan,
                *dry_run,
                manifest_path,
                Verbose::new(*verbose, &crp_diag::Stderr),
            )?;
            Ok(RunOutcome::Apply { message })
        }
    }
}

fn publication_output(verbose: bool) -> PublicationOutput {
    PublicationOutput::new(
        env!("CARGO_PKG_VERSION"),
        verbose,
        Arc::new(crp_diag::Stderr),
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(RunInput: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(RunOutcome: UnwindSafe, RefUnwindSafe);
}
