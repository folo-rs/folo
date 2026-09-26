//! Internal executable and maintainer-test wiring remains reachable through the shell.

use cargo_release_plan::{CheckFormat, Cli, EarlyExit, RunInput, RunOutcome, run};
use ohno::AppError;

::testing::set_allocator!();

#[test]
fn executable_wiring_is_reexported() {
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
