//! Command-line argument parsing for `cargo-release-plan`, built on `clap`.
//!
//! Parsing accepts the argument vector Cargo passes to a subcommand, including
//! the injected `release-plan` token, and yields either a typed [`RunInput`] or
//! an [`EarlyExit`].

use std::ffi::OsString;
use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Error as ClapError, Parser, Subcommand, ValueEnum};

use crate::{CheckFormat, RunInput};

/// Parsed command line, before defaults are resolved.
///
/// This is the tool's argument model: the binary parses argv into it and then
/// converts it into the [`RunInput`] the core logic runs on, so `clap` types do
/// not reach the rest of the crate.
#[derive(Debug, Parser)]
#[command(
    name = "cargo-release-plan",
    about = "Plan workspace releases and publish captured exact versions to crates.io and GitHub.",
    // The implementation partition and application share an exact release version.
    // Installation checks identify that executable, independently of a consumer workspace.
    version = env!("CARGO_PKG_VERSION")
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

impl Cli {
    /// Parses OS arguments, stripping the `release-plan` token Cargo injects.
    ///
    /// # Errors
    ///
    /// Returns an [`EarlyExit`] when the arguments request help/usage or fail to
    /// parse.
    pub fn from_args_os<I, T>(args: I) -> Result<Self, EarlyExit>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let mut argv: Vec<OsString> = args.into_iter().map(Into::into).collect();
        if argv.get(1).is_some_and(|arg| arg == "release-plan") {
            argv.remove(1);
        }
        // Cargo invokes providers with only its plugin marker; configured extra arguments
        // belong to the JSON request, not executable argv.
        if argv.get(1).is_some_and(|arg| arg == "--cargo-plugin") {
            argv.insert(1, OsString::from("credential-provider"));
        }
        Self::try_parse_from(argv).map_err(|error| EarlyExit::from_clap(&error))
    }

    /// Translates the parsed arguments into the [`RunInput`] the core logic consumes.
    ///
    /// CLI-owned defaults such as the workspace manifest path are resolved here.
    /// An absent base remains `None` so execution can use the repository's
    /// recorded remote default branch, falling back to `origin/main`.
    #[must_use]
    pub fn into_input(self) -> RunInput {
        match self.command {
            Command::CheckPublished(args) => RunInput::CheckPublished {
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                plan: args.plan,
                verbose: args.verbose,
            },
            Command::CheckCompatibility(args) => RunInput::CheckCompatibility {
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                prepared: args.prepared,
                plan: args.plan,
                base: args.base,
                output: args.output,
                deny_findings: args.deny_findings,
                verbose: args.verbose,
            },
            Command::Publish(PublishCommand::Report(args)) => RunInput::PublicationReport {
                repository: args.repository,
                publication: args.publication,
                outcomes: args.outcomes,
                jobs: args.jobs,
                output: args.output,
                no_issue: args.no_issue,
            },
            Command::ReleaseContext(args) => RunInput::ReleaseContext {
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                config: args.config,
                base: args.base,
                verbose: args.verbose,
            },
            Command::CheckPublishingIdentity(args) => RunInput::CheckPublishingIdentity {
                verbose: args.verbose,
            },
            Command::Publish(PublishCommand::Binaries(args)) => RunInput::PublishBinaries {
                publication: args.publication,
                batch: args.batch,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                output: args.output,
                artifacts: args.artifacts,
                no_upload: args.no_upload,
            },
            Command::Publish(PublishCommand::Github(args)) => RunInput::PublishGithub {
                publication: args.publication,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                output: args.output,
                batches: args.batches,
                dry_run: args.dry_run,
                verbose: args.verbose,
            },
            Command::Publish(PublishCommand::Registry(args)) => RunInput::PublishRegistry {
                publication: args.publication,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                output: args.output,
                dry_run: args.dry_run,
                verbose: args.verbose,
            },
            Command::CredentialProvider(_) => RunInput::CredentialProvider,
            Command::PreparePublish(args) => RunInput::PreparePublish {
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                config: args.config,
                source: args.source,
                output: args.output,
                verbose: args.verbose,
            },
            Command::InspectPlan(args) => RunInput::InspectPlan {
                plan: args.plan,
                require_resolved: args.require_resolved,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::AnalysisOrder(args) => RunInput::AnalysisOrder {
                report: args.report,
                verbose: args.verbose,
            },
            Command::SemverTargets(args) => RunInput::SemverTargets {
                report: args.report,
                verbose: args.verbose,
            },
            Command::Propose(args) => RunInput::Propose {
                report: args.report,
                decisions: args.decisions,
                out: args.out,
                verbose: args.verbose,
            },
            Command::VerifyPreview(args) => RunInput::VerifyPreview {
                plan: args.plan,
                manifest_path: args.manifest_path,
                verbose: args.verbose,
            },
            Command::Prepare(args) => RunInput::Prepare {
                output: args.output,
                base: args.base,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::Preview(args) => RunInput::Preview {
                plan: args.plan,
                prepared: args.prepared,
                output: args.output,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::Report(args) => RunInput::Report {
                out_dir: args.out_dir,
                base: args.base,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::Check(args) => RunInput::Check {
                base: args.base,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                format: args.format.into(),
                verify_packaging: args.verify_packaging,
                config: args.config,
                verbose: args.verbose,
            },
            Command::Expand(args) => RunInput::Expand {
                plan: args.plan,
                out: args.out,
                preserve_input: args.preserve_input,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
            Command::Apply(args) => RunInput::Apply {
                plan: args.plan,
                dry_run: args.dry_run,
                manifest_path: args
                    .manifest_path
                    .unwrap_or_else(|| PathBuf::from("Cargo.toml")),
                verbose: args.verbose,
            },
        }
    }
}

/// A parse outcome that should terminate the program before execution.
///
/// This is either a help/usage request (success, printed to stdout) or a parse
/// error (failure, printed to stderr).
#[derive(Debug)]
#[expect(
    clippy::exhaustive_structs,
    reason = "handoff struct read directly by the in-crate binary and integration tests"
)]
pub struct EarlyExit {
    /// The rendered message (help text or error) to print.
    pub output: String,
    /// `Ok` for a help/usage request (exit success), `Err` for a parse error.
    pub status: Result<(), ()>,
}

impl EarlyExit {
    /// Classifies a `clap` parse error into the success/failure early-exit shape.
    fn from_clap(error: &ClapError) -> Self {
        let success = matches!(
            error.kind(),
            ErrorKind::DisplayHelp
                | ErrorKind::DisplayVersion
                | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
        Self {
            output: error.to_string(),
            status: if success { Ok(()) } else { Err(()) },
        }
    }
}

/// Clap grammar for the subcommands.
#[derive(Debug, Subcommand)]
enum Command {
    /// Check whether publishable packages are established on crates.io.
    ///
    /// Workspace discovery is advisory. With --plan, missing or unknown results fail the check.
    /// This does not verify Trusted Publisher administration.
    CheckPublished(PublishedArgs),
    /// Collect supported external API comparisons without making semantic decisions.
    ///
    /// Without --prepared or --plan, classify the current workspace against --base or its default.
    CheckCompatibility(CompatibilityArgs),
    /// Resolve configured release-branch history and a workspace-scoped concurrency identity.
    ReleaseContext(ContextArgs),
    /// Exchange GitHub OIDC identity for a short-lived crates.io credential and revoke it.
    CheckPublishingIdentity(IdentityArgs),
    /// Deliver the versions captured in an immutable publication manifest.
    #[command(subcommand)]
    Publish(PublishCommand),
    #[command(hide = true)]
    CredentialProvider(CredentialProviderArgs),
    /// Validate clean merged source and capture an immutable publication manifest.
    PreparePublish(PreparePublishArgs),
    /// Validate an expanded plan and print publication and evidence facts as JSON.
    InspectPlan(InspectPlanArgs),
    /// Print dependency-ordered analysis batches from a report as JSON.
    AnalysisOrder(ArtifactReportArgs),
    /// Print affected consumer-contract package names as a JSON array.
    SemverTargets(ArtifactReportArgs),
    /// Translate supplied semantic decisions into a proposed version plan without inspecting a workspace.
    Propose(ProposeArgs),
    /// Refresh the live lockfile offline and capture prepared evidence before semantic assessment.
    Prepare(PrepareArgs),
    /// Resolve all prospective plan effects offline and capture the state for application.
    Preview(PreviewArgs),
    /// Verify that the retained compatibility workspace still matches the resolved plan.
    VerifyPreview(VerifyPreviewArgs),
    /// Write report.json and per-package diffs for the changes needing a release.
    Report(ReportArgs),
    /// Fail on a release the workspace's manifests cannot support.
    ///
    /// Fails when a publishable package has unreleased changes without a version increment, when
    /// a version group disagrees with itself, when a requirement on another workspace package
    /// does not name the version that package declares, when an exact workspace requirement is
    /// malformed, or when a package whose public API exposes a workspace dependency stays
    /// compatible while that dependency releases a breaking change.
    Check(CheckArgs),
    /// Expand groups without resolution; pass the result through preview before apply.
    Expand(ExpandArgs),
    /// Install captured files without resolution, or make proposed manifest-only edits.
    Apply(ApplyArgs),
}

/// Publication phases share immutable intent, not a mutable version plan.
#[derive(Debug, Subcommand)]
enum PublishCommand {
    /// Report final completeness and create an operator failure issue when required.
    Report(PublicationReportArgs),
    /// Build and publish a frozen platform batch from actual tag commits.
    Binaries(BinaryArgs),
    /// Reconcile crates.io and upload missing versions using Cargo.
    Registry(RegistryArgs),
    /// Reconcile package tags and releases, then emit platform batches for missing binary assets.
    Github(GithubArgs),
}

/// Context resolution does not require a clean or already-merged source checkout.
#[derive(Debug, Parser)]
struct ContextArgs {
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    /// Explicit tested baseline (for example a merge-queue base); otherwise fetch the release branch.
    #[arg(long)]
    base: Option<String>,
    #[arg(long)]
    verbose: bool,
}

/// Setup verification needs only ambient GitHub identity, not workspace source.
#[derive(Debug, Parser)]
struct IdentityArgs {
    #[arg(long)]
    verbose: bool,
}

/// An optional resolved plan narrows and strengthens the registry prerequisite check.
#[derive(Debug, Parser)]
struct PublishedArgs {
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Resolved plan whose publishable targets must have known crates.io identities.
    #[arg(long)]
    plan: Option<PathBuf>,
    #[arg(long)]
    verbose: bool,
}

/// Captured evidence or fresh read-only classification supplies the comparison inputs.
#[derive(Debug, Parser)]
struct CompatibilityArgs {
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Check the captured original prepared inputs.
    #[arg(long,conflicts_with_all=["plan","base"])]
    prepared: Option<PathBuf>,
    /// Check a resolved preview's retained prospective workspace.
    #[arg(long,conflicts_with_all=["prepared","base"])]
    plan: Option<PathBuf>,
    /// Baseline for fresh read-only classification when no evidence artifact is selected.
    #[arg(long)]
    base: Option<String>,
    /// Evidence directory; must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Fail the command when completed comparisons find an insufficient version increment.
    #[arg(long)]
    deny_findings: bool,
    #[arg(long)]
    verbose: bool,
}

/// Failure reporting can proceed even when preparation supplied no usable manifest.
#[derive(Debug, Parser)]
struct PublicationReportArgs {
    /// GitHub repository for the report; must match valid supplied publication intent.
    #[arg(long)]
    repository: String,
    #[arg(long)]
    publication: Option<PathBuf>,
    /// Directory of downloaded attempt artifacts, retaining their subdirectories.
    #[arg(long)]
    outcomes: PathBuf,
    /// JSON file of current workflow job results, preserving failures even without receipts.
    #[arg(long)]
    jobs: PathBuf,
    /// Markdown report file; must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Write the report without creating or updating a GitHub issue.
    #[arg(long)]
    no_issue: bool,
}

/// Binary execution keeps the platform batch, outcome and staged files separate.
#[derive(Debug, Parser)]
struct BinaryArgs {
    #[arg(long)]
    publication: PathBuf,
    /// Frozen platform batch emitted by publish github.
    #[arg(long)]
    batch: PathBuf,
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Structured outcome file; must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Staged artifact directory; must not already exist.
    #[arg(long)]
    artifacts: PathBuf,
    /// Build and stage archives without GitHub queries or uploads.
    #[arg(long)]
    no_upload: bool,
}

/// GitHub reconciliation adds an independently transportable batch directory.
#[derive(Debug, Parser)]
struct GithubArgs {
    /// Immutable publication manifest produced by prepare-publish.
    #[arg(long)]
    publication: PathBuf,
    /// Cargo manifest in the clean publication source.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Structured outcome file; must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Directory for frozen platform batches; must not already exist.
    #[arg(long)]
    batches: PathBuf,
    /// Observe registry availability, tags, releases and assets without remote writes.
    #[arg(long)]
    dry_run: bool,
    /// Explain tag, release, asset and platform-batch decisions.
    #[arg(long)]
    verbose: bool,
}

/// Registry source selection, output and nonpublishing observation mode.
#[derive(Debug, Parser)]
struct RegistryArgs {
    /// Immutable publication manifest produced by prepare-publish.
    #[arg(long)]
    publication: PathBuf,
    /// Cargo manifest in the clean publication source.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Structured outcome file, separate from publication intent; must not already exist.
    #[arg(long)]
    output: PathBuf,
    /// Read remote availability without exchanging credentials or uploading.
    #[arg(long)]
    dry_run: bool,
    /// Explain registry observations and upload selection.
    #[arg(long)]
    verbose: bool,
}

/// Cargo's explicit plugin marker prevents accidental interactive credential requests.
#[derive(Debug, Parser)]
struct CredentialProviderArgs {
    #[arg(long, required = true)]
    cargo_plugin: bool,
}

/// Source selection and artifact destination for publication preparation.
#[derive(Debug, Parser)]
struct PreparePublishArgs {
    /// Cargo manifest in the clean source checkout.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Publication configuration relative to the selected workspace.
    #[arg(long)]
    config: Option<PathBuf>,

    /// Full immutable source commit ID; the source checkout must match it.
    #[arg(long)]
    source: String,

    /// Publication-manifest output file; an existing file must contain identical intent.
    #[arg(long)]
    output: PathBuf,

    /// Explain source and publication validation.
    #[arg(long)]
    verbose: bool,
}

/// Report-only commands never discover or resolve a workspace.
#[derive(Debug, Parser)]
struct ArtifactReportArgs {
    /// Report JSON file or directory containing report.json.
    #[arg(long)]
    report: PathBuf,
    /// Print explanatory selection notes to stderr.
    #[arg(long)]
    verbose: bool,
}

/// Semantic decisions are supplied by the caller, not inferred from report evidence.
#[derive(Debug, Parser)]
struct ProposeArgs {
    /// Report JSON file or directory containing report.json.
    #[arg(long)]
    report: PathBuf,
    /// Change decisions JSON file.
    #[arg(long)]
    decisions: PathBuf,
    /// Destination for the proposed plan JSON.
    #[arg(long)]
    out: PathBuf,
    /// Print explanatory version-resolution notes to stderr.
    #[arg(long)]
    verbose: bool,
}

/// Inspection keeps expanded-plan validation out of workflow adapters.
#[derive(Debug, Parser)]
struct InspectPlanArgs {
    /// Expanded plan to validate before publication checks or evidence collection.
    #[arg(long)]
    plan: PathBuf,
    /// Require a captured preview valid for application to the selected workspace.
    #[arg(long)]
    require_resolved: bool,
    /// Path to the workspace Cargo.toml.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Print explanatory validation notes to stderr.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for preparation before semantic assessment.
#[derive(Debug, Parser)]
struct PrepareArgs {
    /// Directory receiving report.json, diffs/, and prepared.json.
    #[arg(long, visible_alias = "out-dir")]
    output: PathBuf,
    /// Release baseline, defaulting to the remote default branch.
    #[arg(long)]
    base: Option<String>,
    /// Path to the workspace Cargo.toml.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Print explanatory preparation notes.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for proposal-specific offline resolution.
#[derive(Debug, Parser)]
struct PreviewArgs {
    /// Proposed version decisions.
    #[arg(long)]
    plan: PathBuf,
    /// Prepared artifact whose report was assessed.
    #[arg(long)]
    prepared: PathBuf,
    /// Directory receiving plan.json, report.json, diffs/, and retained workspace/.
    #[arg(long)]
    output: PathBuf,
    /// Path to the workspace Cargo.toml.
    #[arg(long)]
    manifest_path: Option<PathBuf>,
    /// Print explanatory expansion and resolver notes.
    #[arg(long)]
    verbose: bool,
}

/// Explicit candidate selection prevents compatibility checks from using the original tree.
#[derive(Debug, Parser)]
struct VerifyPreviewArgs {
    /// Path to the resolved plan JSON.
    #[arg(long)]
    plan: PathBuf,
    /// The `resolved.evidence_manifest_path` emitted by preview.
    #[arg(long)]
    manifest_path: PathBuf,
    /// Print explanatory verification notes.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for `report`.
#[derive(Debug, Parser)]
struct ReportArgs {
    /// Directory that receives `report.json` and `diffs/`.
    #[arg(long)]
    out_dir: PathBuf,

    /// Release baseline whose first-parent line supplies anchors.
    ///
    /// Defaults to the default branch the `origin` remote advertises.
    #[arg(long)]
    base: Option<String>,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Print explanatory notes for each classification decision.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for `check`.
#[derive(Debug, Parser)]
struct CheckArgs {
    /// Release baseline whose first-parent line supplies anchors.
    ///
    /// Defaults to the default branch the `origin` remote advertises.
    #[arg(long)]
    base: Option<String>,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Validate publication settings from this workspace-relative configuration file.
    #[arg(long)]
    config: Option<PathBuf>,

    /// How to render diagnostics.
    #[arg(long, value_enum, default_value_t = CliCheckFormat::Text)]
    format: CliCheckFormat,

    /// Warn when released-content rules diverge from `cargo package --list`.
    ///
    /// Non-gating: a mismatch is printed and the check verdict is unchanged.
    #[arg(long)]
    verify_packaging: bool,

    /// Print explanatory notes for each classification decision.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for `expand`.
#[derive(Debug, Parser)]
struct ExpandArgs {
    /// Path to the plan JSON file to expand.
    #[arg(long)]
    plan: PathBuf,

    /// Path that receives the expanded plan JSON.
    #[arg(long)]
    out: PathBuf,

    /// Reject input/output aliases and stage the output before replacing it.
    #[arg(long)]
    preserve_input: bool,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Print explanatory notes for each expansion decision.
    #[arg(long)]
    verbose: bool,
}

/// Arguments for `apply`.
#[derive(Debug, Parser)]
struct ApplyArgs {
    /// Path to the plan JSON file to apply.
    #[arg(long)]
    plan: PathBuf,

    /// Validate and describe planned writes without changing files.
    #[arg(long)]
    dry_run: bool,

    /// Path to the workspace `Cargo.toml`.
    #[arg(long)]
    manifest_path: Option<PathBuf>,

    /// Print explanatory notes for each edit decision.
    #[arg(long)]
    verbose: bool,
}

/// Clap value for `--format`; converted to [`CheckFormat`].
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum CliCheckFormat {
    Text,
    Github,
}

impl From<CliCheckFormat> for CheckFormat {
    fn from(value: CliCheckFormat) -> Self {
        match value {
            CliCheckFormat::Text => Self::Text,
            CliCheckFormat::Github => Self::Github,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};
    use std::path::Path;

    use static_assertions::assert_impl_all;

    use super::{Cli, EarlyExit};
    use crate::RunInput;

    assert_impl_all!(Cli: UnwindSafe, RefUnwindSafe);
    assert_impl_all!(EarlyExit: UnwindSafe, RefUnwindSafe);

    #[test]
    fn from_args_os_strips_cargo_injected_subcommand() {
        let cli = Cli::from_args_os(["cargo-release-plan", "release-plan", "check"]).unwrap();
        match cli.into_input() {
            RunInput::Check { .. } => {}
            other => panic!("expected check, got {other:?}"),
        }
    }

    #[test]
    fn version_identifies_the_application_without_a_workspace() {
        let cases: &[&[&str]] = &[
            &["cargo-release-plan", "--version"],
            &["cargo-release-plan", "-V"],
            &["cargo-release-plan", "release-plan", "--version"],
            &["cargo-release-plan", "release-plan", "-V"],
        ];
        for args in cases {
            let exit = Cli::from_args_os(args.iter().copied()).unwrap_err();
            exit.status.unwrap();
            assert_eq!(
                exit.output.trim(),
                format!("cargo-release-plan {}", env!("CARGO_PKG_VERSION"))
            );
        }
    }

    #[test]
    fn cargo_plugin_marker_selects_the_internal_provider() {
        let cli = Cli::from_args_os(["cargo-release-plan", "--cargo-plugin"]).unwrap();
        assert!(matches!(cli.into_input(), RunInput::CredentialProvider));
    }

    #[test]
    fn compatibility_defaults_to_fresh_advisory_evidence() {
        let cli = Cli::from_args_os([
            "cargo-release-plan",
            "check-compatibility",
            "--output",
            "evidence",
        ])
        .unwrap();
        let RunInput::CheckCompatibility {
            manifest_path,
            prepared,
            plan,
            base,
            output,
            deny_findings,
            verbose,
        } = cli.into_input()
        else {
            panic!()
        };
        assert_eq!(manifest_path, Path::new("Cargo.toml"));
        assert_eq!(output, Path::new("evidence"));
        assert!(prepared.is_none());
        assert!(plan.is_none());
        assert!(base.is_none());
        assert!(!deny_findings);
        assert!(!verbose);
    }

    #[test]
    fn compatibility_preserves_each_exclusive_source_and_execution_option() {
        for option in ["--prepared", "--plan", "--base"] {
            let cli = Cli::from_args_os([
                "cargo-release-plan",
                "release-plan",
                "check-compatibility",
                option,
                "selected-source",
                "--manifest-path",
                "selected-manifest",
                "--output",
                "new-evidence",
                "--deny-findings",
                "--verbose",
            ])
            .unwrap();
            let RunInput::CheckCompatibility {
                manifest_path,
                prepared,
                plan,
                base,
                output,
                deny_findings,
                verbose,
            } = cli.into_input()
            else {
                panic!()
            };
            assert_eq!(manifest_path, Path::new("selected-manifest"));
            assert_eq!(output, Path::new("new-evidence"));
            assert_eq!(
                prepared.as_deref(),
                (option == "--prepared").then_some(Path::new("selected-source"))
            );
            assert_eq!(
                plan.as_deref(),
                (option == "--plan").then_some(Path::new("selected-source"))
            );
            assert_eq!(
                base.as_deref(),
                (option == "--base").then_some("selected-source")
            );
            assert!(deny_findings);
            assert!(verbose);
        }
    }

    #[test]
    fn compatibility_rejects_ambiguous_sources_and_missing_output() {
        for options in [
            vec!["--prepared", "prepared", "--plan", "plan"],
            vec!["--prepared", "prepared", "--base", "base"],
            vec!["--plan", "plan", "--base", "base"],
        ] {
            let mut args = vec![
                "cargo-release-plan",
                "check-compatibility",
                "--output",
                "evidence",
            ];
            args.extend(options);
            Cli::from_args_os(args).unwrap_err().status.unwrap_err();
        }
        Cli::from_args_os(["cargo-release-plan", "check-compatibility"])
            .unwrap_err()
            .status
            .unwrap_err();
    }

    #[test]
    fn published_discovery_and_resolved_plan_gate_remain_distinct() {
        for explicit in [false, true] {
            let mut args = vec!["cargo-release-plan", "check-published"];
            if explicit {
                args.extend([
                    "--manifest-path",
                    "selected-manifest",
                    "--plan",
                    "resolved-plan",
                    "--verbose",
                ]);
            }
            let RunInput::CheckPublished {
                manifest_path,
                plan,
                verbose,
            } = Cli::from_args_os(args).unwrap().into_input()
            else {
                panic!()
            };
            assert_eq!(
                manifest_path,
                Path::new(if explicit {
                    "selected-manifest"
                } else {
                    "Cargo.toml"
                })
            );
            assert_eq!(
                plan.as_deref(),
                explicit.then_some(Path::new("resolved-plan"))
            );
            assert_eq!(verbose, explicit);
        }
    }
    #[test]
    fn release_context_retains_default_and_explicit_workspace_policy() {
        for explicit in [false, true] {
            let mut args = vec!["cargo-release-plan", "release-context"];
            if explicit {
                args.extend([
                    "--manifest-path",
                    "selected-manifest",
                    "--config",
                    "selected-config",
                    "--base",
                    "tested-base",
                    "--verbose",
                ]);
            }
            let RunInput::ReleaseContext {
                manifest_path,
                config,
                base,
                verbose,
            } = Cli::from_args_os(args).unwrap().into_input()
            else {
                panic!()
            };
            assert_eq!(
                manifest_path,
                Path::new(if explicit {
                    "selected-manifest"
                } else {
                    "Cargo.toml"
                })
            );
            assert_eq!(
                config.as_deref(),
                explicit.then_some(Path::new("selected-config"))
            );
            assert_eq!(base.as_deref(), explicit.then_some("tested-base"));
            assert_eq!(verbose, explicit);
        }
    }

    #[test]
    fn publication_preparation_preserves_source_and_configuration() {
        for explicit in [false, true] {
            let mut args = vec![
                "cargo-release-plan",
                "prepare-publish",
                "--source",
                "immutable-commit",
                "--output",
                "publication",
            ];
            if explicit {
                args.extend([
                    "--manifest-path",
                    "selected-manifest",
                    "--config",
                    "selected-config",
                    "--verbose",
                ]);
            }
            let RunInput::PreparePublish {
                manifest_path,
                config,
                source,
                output,
                verbose,
            } = Cli::from_args_os(args).unwrap().into_input()
            else {
                panic!()
            };
            assert_eq!(
                manifest_path,
                Path::new(if explicit {
                    "selected-manifest"
                } else {
                    "Cargo.toml"
                })
            );
            assert_eq!(
                config.as_deref(),
                explicit.then_some(Path::new("selected-config"))
            );
            assert_eq!(source, "immutable-commit");
            assert_eq!(output, Path::new("publication"));
            assert_eq!(verbose, explicit);
        }
    }

    #[test]
    fn registry_and_github_options_keep_the_frozen_input_separate_from_outputs() {
        for phase in ["registry", "github"] {
            for explicit in [false, true] {
                let mut args = vec![
                    "cargo-release-plan",
                    "publish",
                    phase,
                    "--publication",
                    "immutable-publication",
                    "--output",
                    "phase-outcome",
                ];
                if phase == "github" {
                    args.extend(["--batches", "native-batches"]);
                }
                if explicit {
                    args.extend([
                        "--manifest-path",
                        "selected-manifest",
                        "--dry-run",
                        "--verbose",
                    ]);
                }
                let input = Cli::from_args_os(args).unwrap().into_input();
                match &input {
                    RunInput::PublishGithub { batches, .. } => {
                        assert_eq!(phase, "github");
                        assert_eq!(batches, Path::new("native-batches"));
                    }
                    RunInput::PublishRegistry { .. } => assert_eq!(phase, "registry"),
                    _ => panic!(),
                }
                let (RunInput::PublishRegistry {
                    publication,
                    manifest_path,
                    output,
                    dry_run,
                    verbose,
                }
                | RunInput::PublishGithub {
                    publication,
                    manifest_path,
                    output,
                    dry_run,
                    verbose,
                    ..
                }) = input
                else {
                    panic!()
                };
                assert_eq!(publication, Path::new("immutable-publication"));
                assert_eq!(output, Path::new("phase-outcome"));
                assert_eq!(
                    manifest_path,
                    Path::new(if explicit {
                        "selected-manifest"
                    } else {
                        "Cargo.toml"
                    })
                );
                assert_eq!(dry_run, explicit);
                assert_eq!(verbose, explicit);
            }
        }
    }

    #[test]
    fn identity_probe_preserves_verbose_selection() {
        for verbose in [false, true] {
            let mut args = vec!["cargo-release-plan", "check-publishing-identity"];
            if verbose {
                args.push("--verbose");
            }
            assert!(matches!(Cli::from_args_os(args).unwrap().into_input(),
                RunInput::CheckPublishingIdentity { verbose: actual } if actual == verbose));
        }
    }

    #[test]
    fn publication_report_preserves_inputs_and_issue_selection() {
        let required = [
            ("--repository", "example/repository"),
            ("--outcomes", "receipts"),
            ("--jobs", "job-results"),
            ("--output", "report"),
        ];
        for missing in 0..required.len() {
            let mut args = vec!["cargo-release-plan", "publish", "report"];
            for (index, (flag, value)) in required.iter().enumerate() {
                if index != missing {
                    args.extend([*flag, *value]);
                }
            }
            Cli::from_args_os(args).unwrap_err().status.unwrap_err();
        }
        for explicit in [false, true] {
            let mut args = vec!["cargo-release-plan", "publish", "report"];
            for pair in required {
                args.extend(<[&str; 2]>::from(pair));
            }
            if explicit {
                args.extend(["--publication", "intent", "--no-issue"]);
            }
            let RunInput::PublicationReport {
                repository,
                publication,
                outcomes,
                jobs,
                output,
                no_issue,
            } = Cli::from_args_os(args).unwrap().into_input()
            else {
                panic!()
            };
            assert_eq!(repository, "example/repository");
            assert_eq!(
                publication.as_deref(),
                explicit.then_some(Path::new("intent"))
            );
            assert_eq!(outcomes, Path::new("receipts"));
            assert_eq!(jobs, Path::new("job-results"));
            assert_eq!(output, Path::new("report"));
            assert_eq!(no_issue, explicit);
        }
    }

    #[test]
    fn binary_publication_preserves_paths_manifest_and_upload_selection() {
        let required = [
            ("--publication", "intent"),
            ("--batch", "platform-batch"),
            ("--output", "outcome"),
            ("--artifacts", "artifacts"),
        ];
        for missing in 0..required.len() {
            let mut args = vec!["cargo-release-plan", "publish", "binaries"];
            for (index, (flag, value)) in required.iter().enumerate() {
                if index != missing {
                    args.extend([*flag, *value]);
                }
            }
            Cli::from_args_os(args).unwrap_err().status.unwrap_err();
        }
        for explicit in [false, true] {
            let mut args = vec!["cargo-release-plan", "publish", "binaries"];
            for pair in required {
                args.extend(<[&str; 2]>::from(pair));
            }
            if explicit {
                args.extend(["--manifest-path", "source/Cargo.toml", "--no-upload"]);
            }
            let RunInput::PublishBinaries {
                publication,
                batch,
                manifest_path,
                output,
                artifacts,
                no_upload,
            } = Cli::from_args_os(args).unwrap().into_input()
            else {
                panic!()
            };
            assert_eq!(publication, Path::new("intent"));
            assert_eq!(batch, Path::new("platform-batch"));
            assert_eq!(
                manifest_path,
                Path::new(if explicit {
                    "source/Cargo.toml"
                } else {
                    "Cargo.toml"
                })
            );
            assert_eq!(output, Path::new("outcome"));
            assert_eq!(artifacts, Path::new("artifacts"));
            assert_eq!(no_upload, explicit);
        }
    }

    #[test]
    fn publication_phase_specific_options_do_not_leak_between_commands() {
        for args in [
            vec![
                "cargo-release-plan",
                "publish",
                "registry",
                "--publication",
                "intent",
                "--output",
                "outcome",
                "--batches",
                "batches",
            ],
            vec![
                "cargo-release-plan",
                "publish",
                "github",
                "--publication",
                "intent",
                "--output",
                "outcome",
            ],
        ] {
            Cli::from_args_os(args).unwrap_err().status.unwrap_err();
        }
    }
}
