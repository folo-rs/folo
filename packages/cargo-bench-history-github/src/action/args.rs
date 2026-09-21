use std::path::PathBuf;

use clap::Args;

/// Bootstrap-to-companion handoff for input, output and executable locations.
#[derive(Args, Debug)]
pub(crate) struct ActionArgs {
    /// Strict JSON object of string action inputs, relative to the invocation directory.
    #[arg(long)]
    pub(crate) inputs_file: PathBuf,
    /// Append successful outputs; relative paths use the measured working directory.
    #[arg(long)]
    pub(crate) github_output: PathBuf,
    /// Existing temporary root outside the checkout, relative to the measured directory.
    #[arg(long)]
    pub(crate) temp_dir: PathBuf,
    /// Main executable, relative to the measured directory; omitted means PATH lookup.
    #[arg(long)]
    pub(crate) tool: Option<PathBuf>,
}
