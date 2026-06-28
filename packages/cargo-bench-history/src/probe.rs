//! The environment probe port: discovering git and toolchain facts about the run.
//!
//! The real adapter shells out to `git` and `rustc`; an in-memory fake (in
//! `#[cfg(test)]`) returns canned facts so orchestration is testable.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};

use crate::git::parse_git_info;
use crate::host::{RustcInfo, parse_rustc_verbose};
use crate::machine::{HardwareProfile, system_profile};
use crate::model::GitInfo;
use crate::process::capture;

/// Discovers the git and toolchain context of a run.
pub(crate) trait EnvironmentProbe {
    /// Captures the git facts of the working directory.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected probe failure; a missing repository
    /// or absent `git` yields empty facts rather than an error.
    fn git(&self) -> impl Future<Output = io::Result<GitInfo>>;

    /// Captures the active toolchain's version and host triple.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected probe failure; an absent `rustc`
    /// falls back to a host triple derived from the running platform.
    fn toolchain(&self) -> impl Future<Output = io::Result<RustcInfo>>;

    /// Profiles the host hardware for the machine fingerprint. Best effort:
    /// undetectable facts are left absent rather than failing the probe.
    fn hardware(&self) -> impl Future<Output = HardwareProfile>;
}

/// The real [`EnvironmentProbe`], backed by `git` and `rustc`.
///
/// By default `git` is queried in the process working directory. `in_dir`
/// targets a specific repository directory (via `git -C <dir>`) so a run can
/// probe a checked-out worktree without mutating the process current directory.
#[derive(Clone, Debug, Default)]
pub(crate) struct SystemProbe {
    /// Repository directory passed to `git -C`; the process CWD when absent.
    repo: Option<PathBuf>,
}

impl SystemProbe {
    /// Probes `git` in `repo` (via `git -C <repo>`) instead of the process CWD.
    pub(crate) fn in_dir(repo: impl Into<PathBuf>) -> Self {
        Self {
            repo: Some(repo.into()),
        }
    }

    /// Captures one git field, yielding the empty string on any failure.
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
    async fn git_field(&self, args: &[&str]) -> String {
        let repo = self
            .repo
            .as_deref()
            .map(Path::to_string_lossy)
            .map(std::borrow::Cow::into_owned);
        let mut full: Vec<&str> = Vec::new();
        if let Some(repo) = repo.as_deref() {
            full.push("-C");
            full.push(repo);
        }
        full.extend_from_slice(args);
        match capture("git", &full).await {
            Ok(output) if output.status.success() => output.stdout,
            _ => String::new(),
        }
    }
}

impl EnvironmentProbe for SystemProbe {
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; the parsing it delegates to is tested.
    async fn git(&self) -> io::Result<GitInfo> {
        let commit = self.git_field(&["rev-parse", "HEAD"]).await;
        let branch = self.git_field(&["rev-parse", "--abbrev-ref", "HEAD"]).await;
        let status = self.git_field(&["status", "--porcelain"]).await;

        Ok(parse_git_info(&commit, &branch, &status))
    }

    #[cfg_attr(test, mutants::skip)] // Shells out to `rustc`; the parsing it delegates to is tested.
    async fn toolchain(&self) -> io::Result<RustcInfo> {
        let output = capture("rustc", &["-vV"]).await.ok();
        let rustc_output = output
            .as_ref()
            .filter(|output| output.status.success())
            .map(|output| output.stdout.as_str());
        Ok(resolve_toolchain(rustc_output))
    }

    #[cfg_attr(test, mutants::skip)] // Queries the host hardware; the fingerprint logic is tested.
    async fn hardware(&self) -> HardwareProfile {
        system_profile().await
    }
}

/// Builds the toolchain facts from optional `rustc -vV` output, filling in a
/// platform-derived host triple when `rustc` did not report one.
fn resolve_toolchain(rustc_output: Option<&str>) -> RustcInfo {
    let mut info = rustc_output.map_or_else(RustcInfo::default, parse_rustc_verbose);
    if info.host.is_none() {
        info.host = Some(fallback_host_triple());
    }
    info
}

/// Best-effort host triple derived from the running platform, used when `rustc`
/// cannot be queried.
fn fallback_host_triple() -> String {
    use std::env::consts;

    host_triple_for(consts::ARCH, consts::OS)
}

/// Maps an `(arch, os)` pair to a conventional Rust target triple (pure).
fn host_triple_for(arch: &str, os: &str) -> String {
    match os {
        "windows" => format!("{arch}-pc-windows-msvc"),
        "macos" => format!("{arch}-apple-darwin"),
        "linux" => format!("{arch}-unknown-linux-gnu"),
        other => format!("{arch}-unknown-{other}"),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::env::consts;
    use std::path::PathBuf;

    use super::{SystemProbe, fallback_host_triple, host_triple_for, resolve_toolchain};

    #[test]
    fn in_dir_targets_the_repository_directory() {
        // `in_dir` must record the repository directory so git is queried with
        // `-C <repo>`; the default probe leaves it absent (git runs in the
        // process CWD), so the two must differ.
        let probe = SystemProbe::in_dir("some/repo");
        assert_eq!(probe.repo, Some(PathBuf::from("some/repo")));
        assert_eq!(SystemProbe::default().repo, None);
    }

    #[test]
    fn fallback_host_triple_describes_the_running_platform() {
        let triple = fallback_host_triple();

        assert!(triple.starts_with(consts::ARCH), "{triple}");
        if cfg!(target_os = "windows") {
            assert!(triple.ends_with("-pc-windows-msvc"), "{triple}");
        } else if cfg!(target_os = "macos") {
            assert!(triple.ends_with("-apple-darwin"), "{triple}");
        } else if cfg!(target_os = "linux") {
            assert!(triple.ends_with("-unknown-linux-gnu"), "{triple}");
        }
    }

    #[test]
    fn host_triple_for_covers_each_platform() {
        assert_eq!(
            host_triple_for("x86_64", "windows"),
            "x86_64-pc-windows-msvc"
        );
        assert_eq!(host_triple_for("aarch64", "macos"), "aarch64-apple-darwin");
        assert_eq!(
            host_triple_for("x86_64", "linux"),
            "x86_64-unknown-linux-gnu"
        );
        assert_eq!(host_triple_for("riscv64", "redox"), "riscv64-unknown-redox");
    }

    #[test]
    fn resolve_toolchain_parses_rustc_output() {
        let info = resolve_toolchain(Some("release: 1.91.0\nhost: x86_64-unknown-linux-gnu\n"));
        assert_eq!(info.version.as_deref(), Some("1.91.0"));
        assert_eq!(info.host.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn resolve_toolchain_without_output_falls_back_to_platform_host() {
        let info = resolve_toolchain(None);
        assert_eq!(info.version, None);
        assert_eq!(info.host, Some(fallback_host_triple()));
    }

    #[test]
    fn resolve_toolchain_fills_missing_host_from_platform() {
        let info = resolve_toolchain(Some("release: 1.91.0\n"));
        assert_eq!(info.version.as_deref(), Some("1.91.0"));
        assert_eq!(info.host, Some(fallback_host_triple()));
    }
}
