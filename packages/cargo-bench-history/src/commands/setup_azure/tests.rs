#![allow(
    clippy::indexing_slicing,
    reason = "fixture indexes intentionally fail on a missing value"
)]

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::ffi::OsString;
use std::future::{Future, ready};
use std::io;
use std::panic::{RefUnwindSafe, UnwindSafe};
use std::path::{Path, PathBuf};

use cbh_command::{LocalPrincipalType, SetupAzureOptions};
use cbh_diag::RecordingReporter;
use futures::executor::block_on;
use serde_json::{Value, from_str};
use static_assertions::assert_impl_all;

use crate::commands::setup_azure::bundle::{BundleFile, prepare};
use crate::commands::setup_azure::errors::{
    SetupCleanupError, SetupParameterError, SetupProcessError, SetupTemporaryDirectoryError,
};
use crate::commands::setup_azure::execute::execute_with;
use crate::commands::setup_azure::ports::{BundleFiles, ProcessOutput, SetupProcess};

assert_impl_all!(SetupAzureOptions: UnwindSafe, RefUnwindSafe);
assert_impl_all!(LocalPrincipalType: UnwindSafe, RefUnwindSafe);

/// In-memory bundle ownership with ordered events shared by the process fake.
#[derive(Default)]
struct FakeFiles {
    events: RefCell<Vec<&'static str>>,
    files: RefCell<BTreeMap<String, String>>,
    fail_temporary: bool,
    fail_populate: bool,
    fail_cleanup: bool,
}

impl BundleFiles for FakeFiles {
    type Temporary = PathBuf;

    fn export(&self, _: &Path, files: &[BundleFile]) -> impl Future<Output = io::Result<()>> {
        self.events.borrow_mut().push("export");
        self.save(files);
        ready(Ok(()))
    }

    fn temporary(&self) -> impl Future<Output = io::Result<PathBuf>> {
        self.events.borrow_mut().push("temporary");
        if self.fail_temporary {
            return ready(Err(io::Error::other("temporary canary")));
        }
        ready(Ok(PathBuf::from("owned space ' bundle")))
    }

    fn populate(&self, _: &Path, files: &[BundleFile]) -> impl Future<Output = io::Result<()>> {
        self.events.borrow_mut().push("populate");
        if self.fail_populate {
            return ready(Err(io::Error::other("write canary")));
        }
        self.save(files);
        ready(Ok(()))
    }

    fn cleanup(&self, _: PathBuf) -> impl Future<Output = io::Result<()>> {
        self.events.borrow_mut().push("cleanup");
        if self.fail_cleanup {
            return ready(Err(io::Error::other("cleanup canary")));
        }
        ready(Ok(()))
    }
}

impl FakeFiles {
    fn save(&self, files: &[BundleFile]) {
        self.files.borrow_mut().extend(
            files
                .iter()
                .map(|file| (file.name.to_owned(), file.contents.clone())),
        );
    }
}

/// Records structural argv and returns queued outcomes without launching anything.
struct FakeProcess<'a> {
    files: &'a FakeFiles,
    arguments: RefCell<Vec<Vec<OsString>>>,
    outcomes: RefCell<VecDeque<io::Result<ProcessOutput>>>,
}

impl SetupProcess for FakeProcess<'_> {
    fn run(&self, arguments: &[OsString]) -> impl Future<Output = io::Result<ProcessOutput>> {
        self.files.events.borrow_mut().push("process");
        self.arguments.borrow_mut().push(arguments.to_vec());
        ready(self.outcomes.borrow_mut().pop_front().unwrap())
    }
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "fixture outcomes share the fallible process-port result shape"
)]
fn output(success: bool, stdout: &str, stderr: &str) -> io::Result<ProcessOutput> {
    Ok(ProcessOutput {
        success,
        code: Some(if success { 0 } else { 23 }),
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
    })
}

fn options() -> SetupAzureOptions {
    SetupAzureOptions {
        subscription_id: Some("00000000-0000-0000-0000-000000000001".into()),
        resource_group: Some("history-group".into()),
        location: Some("westeurope".into()),
        storage_account: Some("examplehistory".into()),
        github_owner: Some("owner".into()),
        github_repository: Some("repository".into()),
        history_branch: Some("main".into()),
        ..SetupAzureOptions::default()
    }
}

fn process(files: &FakeFiles, outcomes: Vec<io::Result<ProcessOutput>>) -> FakeProcess<'_> {
    FakeProcess {
        files,
        arguments: RefCell::default(),
        outcomes: RefCell::new(outcomes.into()),
    }
}

#[test]
fn export_bypasses_every_process_and_temporary_operation() {
    let files = FakeFiles::default();
    let process = process(&files, vec![]);
    block_on(execute_with(
        &SetupAzureOptions {
            out_dir: Some("export".into()),
            ..SetupAzureOptions::default()
        },
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap();
    assert_eq!(*files.events.borrow(), ["export"]);
    let parameters: Value = from_str(&files.files.borrow()["parameters.json"]).unwrap();
    assert!(parameters["SubscriptionId"].is_null());
    assert!(parameters["ManagedIdentityName"].is_null());
    assert_eq!(parameters["HistoryContainerName"], "bench-history");
}

#[test]
fn supplied_parameters_are_literal_json_data() {
    let mut options = options();
    options.resource_group = Some("group'\"$()\\value".into());
    options.local_principal_id = Some("local-id".into());
    options.local_principal_type = Some(LocalPrincipalType::Group);
    options.managed_identity = Some("custom-identity".into());
    options.container = Some("custom-history".into());
    let files = prepare(&options).unwrap();
    let parameters: Value = from_str(
        &files
            .iter()
            .find(|file| file.name == "parameters.json")
            .unwrap()
            .contents,
    )
    .unwrap();
    assert_eq!(parameters["ResourceGroup"], "group'\"$()\\value");
    assert_eq!(parameters["LocalPrincipalType"], "Group");
    assert_eq!(parameters["LocalPrincipalId"], "local-id");
    assert_eq!(parameters["ManagedIdentityName"], "custom-identity");
    assert_eq!(parameters["HistoryContainerName"], "custom-history");
    assert_eq!(parameters["HistoryBranch"], "main");
}

#[test]
fn prerequisites_precede_materialization_and_driver_argv_is_structural() {
    let files = FakeFiles::default();
    let process = process(
        &files,
        vec![
            output(true, "", ""),
            output(true, "identifiers", "diagnostic"),
        ],
    );
    let mut options = options();
    options.verbose = true;
    let result = block_on(execute_with(
        &options,
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap();
    assert_eq!(result.stdout_text(), Some("identifiersdiagnostic"));
    assert_eq!(
        *files.events.borrow(),
        ["process", "temporary", "populate", "process", "cleanup"]
    );
    assert_eq!(
        process.arguments.borrow()[1],
        [
            OsString::from("-NoProfile"),
            OsString::from("-NonInteractive"),
            OsString::from("-File"),
            Path::new("owned space ' bundle")
                .join("deploy.ps1")
                .into_os_string(),
            OsString::from("-ParametersFile"),
            Path::new("owned space ' bundle")
                .join("parameters.json")
                .into_os_string(),
            OsString::from("-Verbose"),
        ]
    );
}

#[test]
fn prerequisite_failure_does_not_create_a_bundle() {
    let files = FakeFiles::default();
    let process = process(&files, vec![output(false, "version canary", "old version")]);
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupProcessError>().is_some());
    assert_eq!(*files.events.borrow(), ["process"]);
}

#[test]
fn child_failure_preserves_diagnostics_and_cleans_owned_directory() {
    let files = FakeFiles::default();
    let process = process(
        &files,
        vec![
            output(true, "", ""),
            output(false, "partial canary", "Azure canary"),
        ],
    );
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupProcessError>().is_some());
    assert!(error.to_string().contains("partial canary"));
    assert!(error.to_string().contains("Azure canary"));
    assert_eq!(files.events.borrow().last(), Some(&"cleanup"));
}

#[test]
fn write_failure_also_cleans_and_never_deploys() {
    let files = FakeFiles {
        fail_populate: true,
        ..FakeFiles::default()
    };
    let process = process(&files, vec![output(true, "", "")]);
    block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert_eq!(
        *files.events.borrow(),
        ["process", "temporary", "populate", "cleanup"]
    );
}

#[test]
fn cleanup_failure_retains_successful_deployment_output() {
    let files = FakeFiles {
        fail_cleanup: true,
        ..FakeFiles::default()
    };
    let process = process(
        &files,
        vec![
            output(true, "", ""),
            output(true, "identifiers canary", "diagnostics canary"),
        ],
    );
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupCleanupError>().is_some());
    assert!(error.find_source::<SetupProcessError>().is_none());
    assert!(error.to_string().contains("identifiers canary"));
    assert!(error.to_string().contains("diagnostics canary"));
}

#[test]
fn cleanup_failure_retains_original_deployment_failure() {
    let files = FakeFiles {
        fail_cleanup: true,
        ..FakeFiles::default()
    };
    let process = process(
        &files,
        vec![output(true, "", ""), output(false, "", "deploy canary")],
    );
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupCleanupError>().is_some());
    assert!(error.find_source::<SetupProcessError>().is_some());
}

#[test]
fn missing_execute_parameter_is_rejected_before_any_io() {
    let files = FakeFiles::default();
    let process = process(&files, vec![]);
    let error = block_on(execute_with(
        &SetupAzureOptions::default(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupParameterError>().is_some());
    assert!(files.events.borrow().is_empty());
}

#[test]
fn invalid_names_and_partial_local_access_are_rejected() {
    for account in ["ab", "UPPER", "with-hyphen", "accountnameistoolongforazure"] {
        let mut options = options();
        options.storage_account = Some(account.into());
        prepare(&options).unwrap_err();
    }

    for container in ["ab", "Upper", "-start", "end-", "two--hyphens", "a b"] {
        let mut options = options();
        options.container = Some(container.into());
        prepare(&options).unwrap_err();
    }
    let mut options = options();
    options.local_principal_id = Some("id".into());
    prepare(&options).unwrap_err();
    options.local_principal_id = None;
    options.local_principal_type = Some(LocalPrincipalType::User);
    prepare(&options).unwrap_err();
}

#[test]
fn blank_supplied_text_is_rejected() {
    let mut options = options();
    options.resource_group = Some(" ".into());
    let error = prepare(&options).unwrap_err();
    assert_eq!(
        error
            .find_source::<SetupParameterError>()
            .unwrap()
            .parameter,
        "ResourceGroup"
    );
}

#[test]
fn nonempty_text_with_control_characters_is_rejected() {
    let mut options = options();
    options.managed_identity = Some("identity\0name".into());
    let error = prepare(&options).unwrap_err();
    assert_eq!(
        error
            .find_source::<SetupParameterError>()
            .unwrap()
            .parameter,
        "ManagedIdentityName"
    );
}

#[test]
fn missing_powershell_retains_the_process_io_cause_without_creating_files() {
    let files = FakeFiles::default();
    let process = process(
        &files,
        vec![Err(io::Error::new(io::ErrorKind::NotFound, "pwsh canary"))],
    );
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(error.find_source::<SetupProcessError>().is_some());
    assert_eq!(
        error.find_source::<io::Error>().unwrap().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(*files.events.borrow(), ["process"]);
}

#[test]
fn temporary_creation_failure_never_invokes_the_driver_or_cleanup() {
    let files = FakeFiles {
        fail_temporary: true,
        ..FakeFiles::default()
    };
    let process = process(&files, vec![output(true, "", "")]);
    let error = block_on(execute_with(
        &options(),
        Path::new("invocation"),
        &files,
        &process,
        &RecordingReporter::new(),
    ))
    .unwrap_err();
    assert!(
        error
            .find_source::<SetupTemporaryDirectoryError>()
            .is_some()
    );
    assert_eq!(*files.events.borrow(), ["process", "temporary"]);
}
