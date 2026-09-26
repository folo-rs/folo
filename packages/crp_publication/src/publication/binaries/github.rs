use std::any::type_name;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fmt, thread};

use crp_native::command::{cancelled, capture};
use crp_native::{Native, SourceProvider};
use ohno::AppError;

use crate::PublicationOutput;
use crate::publication::binaries::model::{Asset, Binary, Release};

// Delivery retries retain the existing idempotent-request window and query budget.
const GITHUB_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const QUERY_BUDGET: Duration = Duration::from_secs(300);
/// The upload token is kept separate from the environment supplied to build children.
#[derive(Clone)]
pub struct Github {
    repository: String,
    executable: PathBuf,
    token: Option<OsString>,
    pub(crate) output: PublicationOutput,
}

impl fmt::Debug for Github {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Credential material has no diagnostic representation.
        f.debug_struct(type_name::<Self>())
            .field("repository", &self.repository)
            .field("executable", &self.executable)
            .finish_non_exhaustive()
    }
}

impl Github {
    #[must_use]
    pub fn new(repository: String, output: PublicationOutput) -> Self {
        Self::with_executable(
            repository,
            PathBuf::from("gh"),
            env::var_os("GH_TOKEN").or_else(|| env::var_os("GITHUB_TOKEN")),
            output,
        )
    }

    /// Supplies an explicit process boundary without changing the calling process environment.
    #[must_use]
    pub fn with_executable(
        repository: String,
        executable: PathBuf,
        token: Option<OsString>,
        output: PublicationOutput,
    ) -> Self {
        Self {
            repository,
            executable,
            token,
            output,
        }
    }

    // GH is the existing API client; retrying these idempotent operations cannot duplicate assets.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn invoke(
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
            match capture(
                self.executable.as_os_str(),
                &arguments,
                cwd,
                &environment,
                self.output.diagnostics(),
                deadline,
            ) {
                Ok(output) => return Ok(output),
                Err(error)
                    if !cancelled()
                        && attempt < GITHUB_ATTEMPTS
                        && Native::deadline_after(RETRY_DELAY) < deadline =>
                {
                    self.output.line(format_args!(
                        "GitHub operation attempt {attempt} failed; retrying idempotent request: {error}"
                    ));
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
        self.assets_until(binary, cwd, Native::deadline_after(QUERY_BUDGET))
    }

    #[cfg_attr(test, mutants::skip)] // Native query deadline belongs to its caller's operation budget.
    pub(crate) fn assets_until(
        &self,
        binary: &Binary,
        cwd: &Path,
        deadline: Instant,
    ) -> Result<Vec<Asset>, AppError> {
        let output = self.invoke(
            &crp_native::command::strings(&["release", "view", &binary.tag, "--json", "assets"]),
            cwd,
            deadline,
        )?;
        Ok(serde_json::from_str::<Release>(&output)?.assets)
    }
}

impl SourceProvider for Github {
    fn fetch(&self, controller: &Path, commit: &str, deadline: Instant) -> Result<(), AppError> {
        let mut environment = vec![("GIT_LFS_SKIP_SMUDGE", OsStr::new("1"))];
        if let Some(token) = self.token.as_deref() {
            environment.push(("GH_TOKEN", token));
        }
        capture(
            OsStr::new("git"),
            &crp_native::command::strings(&[
                "-c",
                "credential.helper=",
                "-c",
                "credential.helper=!gh auth git-credential",
                "fetch",
                "--no-tags",
                &format!("https://github.com/{}.git", self.repository),
                commit,
            ]),
            controller,
            &environment,
            self.output.diagnostics(),
            deadline,
        )?;
        Ok(())
    }
}
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    #[test]
    fn github_debug_retains_context_without_credential_material() {
        let github = Github::with_executable(
            "A/B".to_owned(),
            PathBuf::from("X"),
            Some(OsString::from("SECRET")),
            PublicationOutput::new("1.2.3", false, std::sync::Arc::new(crp_diag::Discard)),
        );
        let rendered = format!("{github:?}");
        assert!(rendered.contains("A/B"));
        assert!(rendered.contains("\"X\""));
        assert!(!rendered.contains("SECRET"));
    }
}
