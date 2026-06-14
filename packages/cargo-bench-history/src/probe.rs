//! The environment probe port: discovering git and toolchain facts about the run.
//!
//! The real adapter shells out to `git` and `rustc`; an in-memory fake (in
//! `#[cfg(test)]`) returns canned facts so orchestration is testable.

use std::future::Future;
use std::io;

use crate::git::{GitSnapshot, build_snapshot};
use crate::host::{RustcInfo, parse_rustc_verbose};
use crate::process::capture;

/// Discovers the git and toolchain context of a run.
pub(crate) trait EnvironmentProbe {
    /// Captures the git snapshot of the working directory.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected probe failure; a missing repository
    /// or absent `git` yields an empty snapshot rather than an error.
    fn git(&self) -> impl Future<Output = io::Result<GitSnapshot>>;

    /// Captures the active toolchain's version and host triple.
    ///
    /// # Errors
    ///
    /// Returns an error only on an unexpected probe failure; an absent `rustc`
    /// falls back to a host triple derived from the running platform.
    fn toolchain(&self) -> impl Future<Output = io::Result<RustcInfo>>;
}

/// The real [`EnvironmentProbe`], backed by `git` and `rustc`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemProbe;

impl EnvironmentProbe for SystemProbe {
    async fn git(&self) -> io::Result<GitSnapshot> {
        let commit = git_field(&["rev-parse", "HEAD"]).await;
        let short = git_field(&["rev-parse", "--short", "HEAD"]).await;
        let branch = git_field(&["rev-parse", "--abbrev-ref", "HEAD"]).await;
        let status = git_field(&["status", "--porcelain"]).await;
        let committer = git_field(&["show", "-s", "--format=%cI", "HEAD"]).await;

        Ok(build_snapshot(
            &commit, &short, &branch, &status, &committer,
        ))
    }

    async fn toolchain(&self) -> io::Result<RustcInfo> {
        let mut info = match capture("rustc", &["-vV"]).await {
            Ok(output) if output.status.success() => parse_rustc_verbose(&output.stdout),
            _ => RustcInfo::default(),
        };
        if info.host.is_none() {
            info.host = Some(fallback_host_triple());
        }
        Ok(info)
    }
}

/// Captures one git field, yielding the empty string on any failure.
async fn git_field(args: &[&str]) -> String {
    match capture("git", args).await {
        Ok(output) if output.status.success() => output.stdout,
        _ => String::new(),
    }
}

/// Best-effort host triple derived from the running platform, used when `rustc`
/// cannot be queried.
fn fallback_host_triple() -> String {
    use std::env::consts;

    let arch = consts::ARCH;
    match consts::OS {
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

    use super::fallback_host_triple;

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
}
