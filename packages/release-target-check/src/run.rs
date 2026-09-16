use std::env::args_os;
use std::ffi::OsString;
use std::process::ExitCode;

use ohno::AppError;

use crate::cli::Cli;
use crate::verify::verify;

/// Verifies the candidate selected by this process's command-line arguments.
///
/// Prints the result to stdout on success or the error to stderr on failure,
/// returning the corresponding process exit code.
#[must_use]
// Process arguments and standard streams are integration boundaries. All decisions run below.
#[cfg_attr(test, mutants::skip)]
pub fn run() -> ExitCode {
    run_using(
        args_os().skip(1),
        verify,
        |message| println!("{message}"),
        |error| eprintln!("{error}"),
    )
}

fn run_using(
    arguments: impl IntoIterator<Item = OsString>,
    verify: impl FnOnce(&Cli) -> Result<String, AppError>,
    mut output: impl FnMut(&str),
    mut diagnostic: impl FnMut(&AppError),
) -> ExitCode {
    match Cli::parse(arguments).and_then(|cli| verify(&cli)) {
        Ok(message) => {
            output(&message);
            ExitCode::SUCCESS
        }
        Err(error) => {
            diagnostic(&error);
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::repository::VerificationError;

    fn arguments() -> Vec<OsString> {
        [
            "--manifest-path",
            "candidate.toml",
            "--commit",
            &"a".repeat(40),
            "--release-line",
            &"b".repeat(40),
            "--package",
            "widget@1.2.3",
            "--verbose",
        ]
        .map(OsString::from)
        .to_vec()
    }

    #[test]
    fn verifies_parsed_request_and_reports_success() {
        let mut output = Vec::new();
        let mut called = false;
        let code = run_using(
            arguments(),
            |cli| {
                called = true;
                assert_eq!(cli.manifest_path.to_str().unwrap(), "candidate.toml");
                assert_eq!(cli.commit, "a".repeat(40));
                assert_eq!(cli.release_line, "b".repeat(40));
                assert_eq!(cli.packages.len(), 1);
                assert_eq!(cli.packages.get("widget").unwrap().to_string(), "1.2.3");
                assert!(cli.verbose);
                Ok("verification canary".into())
            },
            |message| output.push(message.to_owned()),
            |_| panic!(),
        );
        assert_eq!(code, ExitCode::SUCCESS);
        assert!(called);
        assert_eq!(output, ["verification canary"]);
    }

    #[test]
    fn forwards_verification_failure_without_success_output() {
        let mut diagnostics = Vec::new();
        let code = run_using(
            arguments(),
            |_| Err(VerificationError::new("failure canary").into()),
            |_| panic!(),
            |error| {
                assert!(error.find_source::<VerificationError>().is_some());
                diagnostics.push(error.to_string());
            },
        );
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(diagnostics.len(), 1);
        assert!(diagnostics.first().unwrap().contains("failure canary"));
    }

    #[test]
    fn invalid_arguments_do_not_start_verification() {
        let mut diagnostics = 0;
        let code = run_using([], |_| panic!(), |_| panic!(), |_| diagnostics += 1);
        assert_eq!(code, ExitCode::FAILURE);
        assert_eq!(diagnostics, 1);
    }
}
