use std::path::PathBuf;

use clap::Args;
use ohno::AppError;

use crate::summary::input::SummaryInput;
use crate::summary::render::{InvalidSummary, render};
use crate::workflow::files::{read_file, write_new};

/// Renders one offline card to a new file for the action to append exactly once.
#[derive(Args, Debug)]
pub(crate) struct SummaryArgs {
    #[arg(long)]
    inputs_file: PathBuf,
    #[arg(long)]
    output_file: PathBuf,
}

/// Keeps file access outside the pure card projection and never reads GitHub credentials.
// Native CLI integration tests cover file wiring; rendering uses in-memory evidence tests.
#[cfg_attr(test, mutants::skip)]
pub(crate) fn workflow_summary(args: &SummaryArgs) -> Result<(), AppError> {
    let input: SummaryInput = serde_json::from_slice(&read_file(&args.inputs_file)?)
        .map_err(InvalidSummary::caused_by)?;
    let parent = args.inputs_file.parent().expect("a file path has a parent");
    let markdown = render(input, |path| read_file(&parent.join(path)))?;
    write_new(&args.output_file, markdown.as_bytes())
}
