use std::path::PathBuf;

use clap::{Args, ValueEnum};

/// The workflow's offline setup invocation, separate from analysis receipt reconciliation.
#[derive(Args, Debug)]
pub(crate) struct PrepareWorkflowArgs {
    #[arg(long, value_enum)]
    pub(crate) flow: Flow,
    /// Strict string-valued workflow configuration, not root-action command inputs.
    #[arg(long)]
    pub(crate) inputs_file: PathBuf,
    #[arg(long)]
    pub(crate) github_output: PathBuf,
}

/// Selects event attribution and the supported scope policy for the calling workflow.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum Flow {
    History,
    Pr,
}
