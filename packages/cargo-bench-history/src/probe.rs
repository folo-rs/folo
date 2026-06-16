//! The environment probe port: discovering git and toolchain facts about the run.
//!
//! The real adapter shells out to `git` and `rustc`; an in-memory fake (in
//! `#[cfg(test)]`) returns canned facts so orchestration is testable.

use std::future::Future;
use std::io;

use crate::git::{GitSnapshot, build_snapshot};
use crate::host::{RustcInfo, parse_rustc_verbose};
use crate::machine::{HardwareProfile, system_profile};
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

    /// Profiles the host hardware for the machine fingerprint. Best effort:
    /// undetectable facts are left absent rather than failing the probe.
    fn hardware(&self) -> impl Future<Output = HardwareProfile>;
}

/// The real [`EnvironmentProbe`], backed by `git` and `rustc`.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct SystemProbe;

impl EnvironmentProbe for SystemProbe {
    #[cfg_attr(test, mutants::skip)] // Shells out to `git`; the parsing it delegates to is tested.
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

/// Captures one git field, yielding the empty string on any failure.
#[cfg_attr(test, mutants::skip)] // Shells out to `git`; environment IO with no pure logic to assert.
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

    use super::{fallback_host_triple, host_triple_for, resolve_toolchain};

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
