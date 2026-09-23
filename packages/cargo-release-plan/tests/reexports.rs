//! The supported library surface remains reachable through the application facade.

use cargo_release_plan::{CheckFormat, Cli, EarlyExit, RunInput, RunOutcome, run};
use ohno::AppError;

#[test]
fn supported_items_are_reexported() {
    let cli: Cli = Cli::from_args_os(["cargo-release-plan", "check"]).unwrap();
    let input: RunInput = cli.into_input();
    assert!(matches!(
        input,
        RunInput::Check {
            format: CheckFormat::Text,
            ..
        }
    ));
    let exit: EarlyExit = Cli::from_args_os(["cargo-release-plan", "--help"]).unwrap_err();
    assert!(exit.status.is_ok());
    let outcome = RunOutcome::Propose {
        message: String::new(),
    };
    assert!(matches!(outcome, RunOutcome::Propose { .. }));
    let _: fn(&RunInput) -> Result<RunOutcome, AppError> = run;
}
