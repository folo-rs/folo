use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fs, thread};

use ohno::AppError;
use tempfile::TempDir;

use crate::archive::Staging;
use crate::batch::Executor;
use crate::command::{cancelled, capture, capture_cleanup};
use crate::model::{Asset, Binary, ITEM_MINUTES, InvalidPlan, Release};
use crate::source::{Metadata, executable};

/// Native state belongs to the controller; source worktrees supply only build inputs.
pub(crate) struct Native {
    controller: PathBuf,
    output: PathBuf,
    target: PathBuf,
    triple: String,
    github: Github,
    source: Option<TempDir>,
    metadata: Option<Metadata>,
    staging: Option<Staging>,
    deadline: Instant,
    first_in_source: bool,
}

/// The upload token is kept separate from the environment supplied to build children.
pub(crate) struct Github {
    repository: String,
    token: Option<OsString>,
}

// Release queries/uploads are idempotent. Match the existing short infrastructure retry window.
const GITHUB_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const QUERY_BUDGET: Duration = Duration::from_secs(300);
const ITEM_BUDGET: Duration = Duration::from_secs(ITEM_MINUTES as u64 * 60);

impl Github {
    pub(crate) fn new(repository: String) -> Self {
        Self {
            repository,
            token: env::var_os("GH_TOKEN").or_else(|| env::var_os("GITHUB_TOKEN")),
        }
    }

    // GH is the existing API client; retrying these idempotent operations cannot duplicate assets.
    #[cfg_attr(test, mutants::skip)]
    fn invoke(
        &self,
        arguments: &[OsString],
        cwd: &Path,
        deadline: Instant,
    ) -> Result<String, AppError> {
        let mut arguments = arguments.to_vec();
        arguments.extend([OsString::from("--repo"), OsString::from(&self.repository)]);
        let environment = self
            .token
            .as_deref()
            .map(|token| ("GH_TOKEN", token))
            .into_iter()
            .collect::<Vec<_>>();
        for attempt in 1..=GITHUB_ATTEMPTS {
            match capture(OsStr::new("gh"), &arguments, cwd, &environment, deadline) {
                Ok(output) => return Ok(output),
                Err(error)
                    if !cancelled()
                        && attempt < GITHUB_ATTEMPTS
                        && deadline_after(RETRY_DELAY) < deadline =>
                {
                    eprintln!(
                        "GitHub operation attempt {attempt} failed; retrying idempotent request: {error}"
                    );
                    thread::sleep(RETRY_DELAY);
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("the final attempt always returns")
    }

    // A missing release is an error: reconciliation must create it before binary planning.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn assets(&self, binary: &Binary, cwd: &Path) -> Result<Vec<Asset>, AppError> {
        self.assets_until(binary, cwd, deadline_after(QUERY_BUDGET))
    }

    #[cfg_attr(test, mutants::skip)] // Native query deadline belongs to its caller's operation budget.
    fn assets_until(
        &self,
        binary: &Binary,
        cwd: &Path,
        deadline: Instant,
    ) -> Result<Vec<Asset>, AppError> {
        let output = self.invoke(
            &strings(&["release", "view", &binary.tag, "--json", "assets"]),
            cwd,
            deadline,
        )?;
        Ok(serde_json::from_str::<Release>(&output)?.assets)
    }
}

impl Native {
    // Source/cached-target discovery requires filesystem and subprocess integration coverage.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn new(
        controller: PathBuf,
        output: PathBuf,
        triple: String,
        repository: String,
    ) -> Result<Self, AppError> {
        // Standard Win32 paths are required by PowerShell's script authorization/file APIs.
        let controller = dunce::canonicalize(controller)?;
        fs::create_dir_all(&output)?;
        let output = dunce::canonicalize(output)?;
        let metadata = capture(
            OsStr::new("cargo"),
            &strings(&[
                "metadata",
                "--locked",
                "--offline",
                "--no-deps",
                "--format-version",
                "1",
            ]),
            &controller,
            &[],
            deadline_after(QUERY_BUDGET),
        )?;
        let metadata: Metadata = serde_json::from_str(&metadata)?;
        let target = metadata.target_directory;
        if !target.is_absolute() {
            return Err(
                InvalidPlan::new("Cargo target directory must be absolute".to_owned()).into(),
            );
        }
        Ok(Self {
            controller,
            output,
            target,
            triple,
            github: Github::new(repository),
            source: None,
            metadata: None,
            staging: None,
            deadline: deadline_after(ITEM_BUDGET),
            first_in_source: false,
        })
    }

    // Every build command uses source cwd and the controller cache, without upload credentials.
    #[cfg_attr(test, mutants::skip)]
    fn source_command(&self, program: &str, arguments: &[OsString]) -> Result<String, AppError> {
        let source = self
            .source
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No source worktree is prepared".to_owned()))?
            .path()
            .join("source");
        capture(
            OsStr::new(program),
            arguments,
            &source,
            &[
                ("CARGO_TARGET_DIR", self.target.as_os_str()),
                ("GIT_LFS_SKIP_SMUDGE", OsStr::new("1")),
            ],
            self.deadline,
        )
    }
}

impl Executor for Native {
    fn cancelled(&self) -> bool {
        cancelled()
    }
    #[cfg_attr(test, mutants::skip)] // GitHub adapter; batch decisions have in-process coverage.
    fn assets(&mut self, binary: &Binary) -> Result<Vec<Asset>, AppError> {
        self.github.assets(binary, &self.controller)
    }

    #[cfg_attr(test, mutants::skip)] // Git/rustup boundary, exercised by pinned-source integration.
    fn prepare(&mut self, binary: &Binary) -> Result<(), AppError> {
        self.deadline = deadline_after(ITEM_BUDGET);
        self.first_in_source = true;
        let environment = [("GIT_LFS_SKIP_SMUDGE", OsStr::new("1"))];
        let object = format!("{}^{{commit}}", binary.source_sha);
        // Try the exact object locally before a bounded fetch; never replace it with moving main.
        if capture(
            OsStr::new("git"),
            &strings(&["cat-file", "-e", &object]),
            &self.controller,
            &environment,
            self.deadline,
        )
        .inspect_err(|error| {
            eprintln!("Exact source object is unavailable locally; fetching it: {error}");
        })
        .is_err()
        {
            capture(
                OsStr::new("git"),
                &strings(&["fetch", "--no-tags", "origin", &binary.source_sha]),
                &self.controller,
                &environment,
                self.deadline,
            )?;
        }
        let source = tempfile::Builder::new()
            .prefix("folo-release-source-")
            .tempdir()?;
        let path = source.path().join("source");
        self.source = Some(source);
        capture(
            OsStr::new("git"),
            &[
                "worktree".into(),
                "add".into(),
                "--detach".into(),
                path.as_os_str().to_owned(),
                binary.source_sha.clone().into(),
            ],
            &self.controller,
            &environment,
            self.deadline,
        )?;
        let head = self.source_command("git", &strings(&["rev-parse", "HEAD"]))?;
        if head.trim() != binary.source_sha {
            return Err(InvalidPlan::new(
                "Source worktree HEAD differs from the release tag".to_owned(),
            )
            .into());
        }
        self.source_command(
            "git",
            &strings(&[
                "ls-files",
                "--error-unmatch",
                "Cargo.lock",
                "rust-toolchain.toml",
            ]),
        )?;
        let adapter = self
            .controller
            .join("scripts")
            .join("release")
            .join("Install-ReleaseSourceToolchain.ps1");
        self.source_command(
            "pwsh",
            &[
                "-NoLogo".into(),
                "-NoProfile".into(),
                "-File".into(),
                adapter.into_os_string(),
                "-Target".into(),
                self.triple.clone().into(),
            ],
        )?;
        let compiler = self.source_command("rustc", &strings(&["--version", "--verbose"]))?;
        let host = compiler
            .lines()
            .find_map(|line| line.strip_prefix("host: "));
        if host != Some(self.triple.as_str()) {
            return Err(InvalidPlan::new(format!(
                "Native compiler host {host:?} does not match {}",
                self.triple
            ))
            .into());
        }
        let metadata = self.source_command(
            "cargo",
            &strings(&["metadata", "--locked", "--no-deps", "--format-version", "1"]),
        )?;
        self.metadata = Some(serde_json::from_str(&metadata)?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Cargo invocation/staging boundary; artifact decisions are pure.
    fn build(&mut self, binary: &Binary) -> Result<(), AppError> {
        if !self.first_in_source {
            self.deadline = deadline_after(ITEM_BUDGET);
        }
        self.first_in_source = false;
        self.staging = None;
        let package = self
            .metadata
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("Missing source metadata".to_owned()))?
            .package(binary)?;
        let messages = self.source_command(
            "cargo",
            &strings(&[
                "build",
                "--release",
                "--locked",
                "--target",
                &self.triple,
                "--package",
                &binary.name,
                "--bin",
                &binary.bin,
                "--message-format=json-render-diagnostics",
            ]),
        )?;
        let executable = executable(&messages, &package.id, &binary.bin)?;
        self.staging = Some(Staging::create(
            &self.output,
            binary,
            &self.triple,
            &executable,
        )?);
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Native archive tools and file hashing use integration fixtures.
    fn package(&mut self, _binary: &Binary) -> Result<(), AppError> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No staged executable".to_owned()))?;
        let filename = staging
            .executable
            .file_name()
            .ok_or_else(|| InvalidPlan::new("Missing binary filename".to_owned()))?;
        #[cfg(windows)]
        let (program, arguments) = (
            "7za",
            vec![
                "a".into(),
                "-tzip".into(),
                staging.archive.as_os_str().to_owned(),
                filename.to_owned(),
            ],
        );
        #[cfg(unix)]
        let (program, arguments) = (
            "zip",
            vec![
                "-q".into(),
                staging.archive.as_os_str().to_owned(),
                filename.to_owned(),
            ],
        );
        capture(
            OsStr::new(program),
            &arguments,
            &staging.directory,
            &[],
            self.deadline,
        )?;
        staging.checksum()
    }

    #[cfg_attr(test, mutants::skip)] // Real GH writes are not used by tests; no-upload omits this.
    fn upload(&mut self, binary: &Binary) -> Result<(), AppError> {
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| InvalidPlan::new("No staged archive".to_owned()))?;
        self.github.invoke(
            &[
                "release".into(),
                "upload".into(),
                binary.tag.clone().into(),
                staging.archive.as_os_str().to_owned(),
                staging.checksum.as_os_str().to_owned(),
                "--clobber".into(),
            ],
            &self.controller,
            self.deadline,
        )?;
        if !binary.complete(
            &self.triple,
            &self
                .github
                .assets_until(binary, &self.controller, self.deadline)?,
        ) {
            return Err(InvalidPlan::new(
                "Upload returned success without both uploaded assets".to_owned(),
            )
            .into());
        }
        Ok(())
    }

    #[cfg_attr(test, mutants::skip)] // Owned-worktree cleanup is a Git/filesystem boundary.
    fn cleanup(&mut self) -> Result<(), AppError> {
        self.metadata = None;
        self.staging = None;
        if let Some(source) = self.source.take() {
            // Even an interrupted add can register a worktree before returning a failure.
            // Always attempt Git cleanup for this owned path, then remove its temporary parent.
            let result = capture_cleanup(
                &[
                    "worktree".into(),
                    "remove".into(),
                    "--force".into(),
                    source.path().join("source").into_os_string(),
                ],
                &self.controller,
                deadline_after(QUERY_BUDGET),
            );
            let cleanup = source.close();
            match (result, cleanup) {
                (Err(operation), Err(cleanup)) => {
                    // Both native operations have failed; retain both diagnostics at this boundary.
                    eprintln!("Source directory cleanup also failed: {cleanup}");
                    return Err(operation);
                }
                (Err(error), Ok(())) => return Err(error),
                (Ok(_), Err(error)) => return Err(error.into()),
                (Ok(_), Ok(())) => {}
            }
        }
        Ok(())
    }
}

pub(crate) fn strings(values: &[&str]) -> Vec<OsString> {
    values.iter().map(OsString::from).collect()
}

// Real deadlines exist only in the native adapter, never in unit-test decisions.
#[cfg_attr(test, mutants::skip)]
fn deadline_after(budget: Duration) -> Instant {
    Instant::now()
        .checked_add(budget)
        .expect("bounded release deadline fits in Instant")
}
