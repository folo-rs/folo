use std::ffi::OsString;
use std::path::Path;

use cbh_command::SetupAzureOptions;
use cbh_diag::{Reporter, ReporterExt, StderrReporter};
use ohno::AppError;

use crate::RunOutcome;
use crate::commands::setup_azure::bundle::prepare;
use crate::commands::setup_azure::errors::{
    SetupBundleError, SetupCleanupError, SetupProcessError, SetupTemporaryDirectoryError,
};
use crate::commands::setup_azure::ports::{
    BundleFiles, ProcessOutput, SetupProcess, TokioBundleFiles, TokioSetupProcess,
};

/// Connects command dispatch to the native bundle, process and diagnostic adapters.
///
/// The lifecycle lives in `execute_with`, letting native execution and in-process
/// orchestration tests share the same export/deployment decisions.
pub(crate) async fn execute(
    options: &SetupAzureOptions,
    working_directory: &Path,
) -> Result<RunOutcome, AppError> {
    execute_with(
        options,
        working_directory,
        &TokioBundleFiles,
        &TokioSetupProcess {
            working_directory: working_directory.to_path_buf(),
        },
        &StderrReporter::new(options.verbose),
    )
    .await
}

/// Runs export or deployment through the supplied filesystem and process ports.
///
/// Used by the native entry point and fake-driven tests. It owns bundle materialization,
/// prerequisite ordering and cleanup; Azure resource policy stays in the standalone driver.
pub(crate) async fn execute_with(
    options: &SetupAzureOptions,
    working_directory: &Path,
    files: &impl BundleFiles,
    process: &impl SetupProcess,
    reporter: &dyn Reporter,
) -> Result<RunOutcome, AppError> {
    let bundle = prepare(options)?;
    if let Some(destination) = &options.out_dir {
        let destination = working_directory.join(destination);
        files
            .export(&destination, &bundle)
            .await
            .map_err(|error| SetupBundleError::caused_by("export", destination.clone(), error))?;
        return Ok(RunOutcome::Completed {
            message: format!(
                "Exported Azure deployment bundle to {}. No tools or credentials were probed.\n\
                 Review README.md and fill required values in parameters.json, then run \
                 pwsh -NoProfile -NonInteractive -File deploy.ps1 from that directory.",
                destination.display()
            ),
        });
    }
    // The static script checks the actual host version, rather than parsing a
    // localized version banner. No user values are interpolated into shell code.
    reporter.note_with(|| {
        "checking PowerShell before extracting the bundle; its standalone driver verifies Azure CLI, installed Bicep and the explicit subscription before cloud mutations".to_owned()
    });
    let probe = [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "if ($PSVersionTable.PSVersion -lt [version]'7.6') { throw 'PowerShell 7.6 or later is required.' }",
    ].map(OsString::from);
    run_process(process, &probe, "PowerShell prerequisite").await?;
    let directory = files
        .temporary()
        .await
        .map_err(SetupTemporaryDirectoryError::caused_by)?;
    let path = directory.as_ref().to_path_buf();
    let result = async {
        files
            .populate(&path, &bundle)
            .await
            .map_err(|error| SetupBundleError::caused_by("materialize", path.clone(), error))?;
        let mut arguments = vec![
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-File"),
            path.join("deploy.ps1").into_os_string(),
            OsString::from("-ParametersFile"),
            path.join("parameters.json").into_os_string(),
        ];
        if options.current_user {
            arguments.push(OsString::from("-CurrentUser"));
        }
        if options.verbose {
            arguments.push(OsString::from("-Verbose"));
        }
        let output = run_process(process, &arguments, "deployment").await?;
        // Successful tools may emit useful native diagnostics to stderr too.
        Ok(format!("{}{}", output.stdout, output.stderr))
    }
    .await;
    if let Err(cleanup) = files.cleanup(directory).await {
        return Err(match result {
            Ok(output) => SetupCleanupError::new(path, cleanup, output).into(),
            Err(error) => SetupCleanupError::caused_by(path, cleanup, String::new(), error).into(),
        });
    }
    result.map(|message| RunOutcome::Completed { message })
}

/// Preserves child diagnostics at the shared prerequisite and deployment boundary.
///
/// Callers name the attempted operation so launch and exit failures remain actionable
/// after bundle cleanup, with captured stdout and stderr attached to the application error.
async fn run_process(
    process: &impl SetupProcess,
    arguments: &[OsString],
    operation: &'static str,
) -> Result<ProcessOutput, AppError> {
    let output = process.run(arguments).await.map_err(|error| {
        SetupProcessError::caused_by(
            operation,
            arguments.to_vec(),
            None,
            String::new(),
            String::new(),
            error,
        )
    })?;
    if !output.success {
        return Err(SetupProcessError::new(
            operation,
            arguments.to_vec(),
            output.code,
            output.stdout,
            output.stderr,
        )
        .into());
    }
    Ok(output)
}
