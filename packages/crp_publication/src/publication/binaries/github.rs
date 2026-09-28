use std::any::type_name;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{env, fmt, thread};

use crp_native::command::{cancelled, capture};
use crp_native::{Native, SourceProvider};
use ohno::AppError;

use crate::PublicationOutput;
use crate::publication::binaries::model::Release;
use crate::publication::binaries::{Asset, Binary};

// Retry idempotent GitHub commands briefly within their caller's deadline. Tag reconciliation
// separately refreshes candidate source evidence between its write attempts.
const GITHUB_ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);
// Initial discovery is a short operation with a conservative allowance for slow native startup.
const QUERY_BUDGET: Duration = Duration::from_secs(300);
/// Observes and delivers GitHub assets and acquires exact release-source commits.
///
/// Upload credentials are scoped to forge and repository acquisition operations, not builds.
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
            Self::upload_token(env::var_os("GH_TOKEN"), env::var_os("GITHUB_TOKEN")),
            output,
        )
    }

    fn upload_token(primary: Option<OsString>, fallback: Option<OsString>) -> Option<OsString> {
        primary.into_iter().chain(fallback).find(|token| {
            token
                .as_encoded_bytes()
                .iter()
                .any(|byte| !byte.is_ascii_whitespace())
        })
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

    // Delivery names the configured repository rather than inferring it from the checkout's remote.
    #[cfg_attr(test, mutants::skip)]
    pub(crate) fn invoke(
        &self,
        arguments: &[OsString],
        cwd: &Path,
        deadline: Instant,
    ) -> Result<String, AppError> {
        let mut arguments = arguments.to_vec();
        arguments.extend([OsString::from("--repo"), OsString::from(&self.repository)]);
        self.invoke_command(&arguments, cwd, deadline)
    }

    #[cfg_attr(test, mutants::skip)] // Supervised GitHub process execution uses native boundary tests.
    fn invoke_command(
        &self,
        arguments: &[OsString],
        cwd: &Path,
        deadline: Instant,
    ) -> Result<String, AppError> {
        let environment = self
            .token
            .as_deref()
            .map(|token| ("GH_TOKEN", token))
            .into_iter()
            .collect::<Vec<_>>();
        for attempt in 1..=GITHUB_ATTEMPTS {
            match capture(
                self.executable.as_os_str(),
                arguments,
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
        // Fetch only the requested immutable commit from the configured repository, not
        // unrelated tag refs. Reset persistent credential helpers before installing the
        // invocation helper, and scope its token to this command rather than build children.
        // The LFS setting prevents incidental smudge activity in acquisition tooling; fetch
        // itself does not populate a working tree and this flag is not process isolation.
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
    use std::io;

    use serde_json::json;

    use super::*;
    use crate::publication::github::peel_tag;

    #[test]
    fn canonical_peeling_follows_nested_annotations_without_substituting_their_ids() {
        let outer = "a".repeat(40);
        let inner = "b".repeat(40);
        let commit = "c".repeat(40);
        let mut visited = Vec::new();
        let resolved = peel_tag(
            "release",
            json!({"object":{"type":"tag","sha":outer}}),
            |suffix| {
                visited.push(suffix.to_owned());
                if suffix == format!("/git/tags/{outer}") {
                    Ok(Some(json!({"object":{"type":"tag","sha":inner}})))
                } else {
                    assert_eq!(suffix, format!("/git/tags/{inner}"));
                    Ok(Some(json!({"object":{"type":"commit","sha":commit}})))
                }
            },
        )
        .unwrap();
        assert_eq!(resolved, commit);
        assert_eq!(
            visited,
            [format!("/git/tags/{outer}"), format!("/git/tags/{inner}")]
        );
    }

    #[test]
    fn canonical_peeling_preserves_query_failure_and_rejects_malformed_observations() {
        let error = peel_tag(
            "release",
            json!({"object":{"type":"tag","sha":"a".repeat(40)}}),
            |_| Err(io::Error::from(io::ErrorKind::ConnectionRefused).into()),
        )
        .unwrap_err();
        assert_eq!(
            error.find_source::<io::Error>().unwrap().kind(),
            io::ErrorKind::ConnectionRefused
        );
        peel_tag("release", json!({}), |_| panic!()).unwrap_err();
    }

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

    #[test]
    fn blank_primary_tokens_do_not_hide_a_usable_fallback() {
        for primary in [None, Some(OsString::new()), Some(" \t\n".into())] {
            assert_eq!(
                Github::upload_token(primary, Some("fallback".into())),
                Some("fallback".into())
            );
        }
        assert_eq!(
            Github::upload_token(Some("primary".into()), Some("fallback".into())),
            Some("primary".into()),
        );
        assert!(Github::upload_token(Some(" ".into()), Some("\t".into())).is_none());
        assert!(Github::upload_token(None, None).is_none());
    }
}
