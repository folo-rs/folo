//! Hermetic Git helpers for cargo-release-plan integration tests.
//!
//! Git configuration is pinned by `Fixture` so tests do not depend on host or
//! user settings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A temporary Git repository that is also a Cargo workspace.
pub(crate) struct Fixture {
    dir: TempDir,
}

impl Fixture {
    /// Creates the repository and writes the workspace manifest.
    ///
    /// `extra` is appended to the root manifest, so a caller can add tables such
    /// as `[workspace.metadata.release-plan.groups]` or `[workspace.package]`.
    pub(crate) fn new(extra: &str) -> Self {
        let dir = TempDir::new().unwrap();
        let fixture = Self { dir };
        fixture.git(&["init", "-b", "main"]);
        fixture.write_workspace(extra);
        fixture
    }

    /// Rewrites the root manifest, replacing the tables `new` appended.
    pub(crate) fn write_workspace(&self, extra: &str) {
        // Ordinary supported manifest revisions so `cargo metadata` accepts the
        // generated workspace. Tests do not cover resolver or edition behavior.
        self.write(
            "Cargo.toml",
            &format!(
                r#"[workspace]
members = ["packages/*"]
resolver = "2"
{extra}
"#
            ),
        );
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn manifest(&self) -> PathBuf {
        self.path().join("Cargo.toml")
    }

    pub(crate) fn write(&self, rel: &str, contents: &str) {
        let path = self.path().join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, contents).unwrap();
    }

    pub(crate) fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.path().join(rel)).unwrap()
    }

    pub(crate) fn git(&self, args: &[&str]) -> String {
        let mut command = hermetic_git();
        command.arg("-C");
        command.arg(self.path());
        command.args(args);
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub(crate) fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }

    /// Runs Cargo against the fixture workspace.
    ///
    /// Offline throughout, since the fixture packages never depend on anything
    /// outside the workspace and a registry lookup would make tests non-hermetic.
    pub(crate) fn cargo(&self, args: &[&str]) -> String {
        let output = Command::new("cargo")
            .args(args)
            .arg("--manifest-path")
            .arg(self.manifest())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "cargo {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    pub(crate) fn sha(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_string()
    }
}

/// Git settings pinned for every invocation.
///
/// No test may inherit host or user configuration: an unset identity, a signing
/// key, or a background `gc` would all make a test depend on the machine it
/// runs on.
const HERMETIC_CONFIG: &[&str] = &[
    "-c",
    "user.email=release-plan@example.invalid",
    "-c",
    "user.name=Release Plan Test",
    "-c",
    "commit.gpgsign=false",
    "-c",
    "gc.auto=0",
    "-c",
    "core.autocrlf=false",
];

/// Empty global configuration source understood by the platform's Git executable.
#[cfg(windows)]
const EMPTY_GIT_CONFIG: &str = "NUL";
/// Empty global configuration source understood by the platform's Git executable.
#[cfg(not(windows))]
const EMPTY_GIT_CONFIG: &str = "/dev/null";

/// A `git` command carrying the pinned configuration and no working directory.
///
/// `Fixture::git` runs inside an existing fixture; a test that creates a
/// repository somewhere else, such as a clone, needs the same settings without
/// one.
pub(crate) fn hermetic_git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", EMPTY_GIT_CONFIG)
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS");
    command.args(HERMETIC_CONFIG);
    command
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn hermetic_git_ignores_the_users_global_configuration() {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join(".gitconfig"),
        "[core]\nhooksPath = unwanted-hooks\n",
    )
    .unwrap();
    let output = hermetic_git()
        .env("HOME", home.path())
        .args(["config", "--global", "--get", "core.hooksPath"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
}

/// Writes a package whose only target is an executable.
///
/// A binary package releases the dependency closure its archive's lockfile
/// records, which a library package does not.
/// Ref: docs/design.md, "Lockfiles of binary packages".
pub(crate) fn write_binary_package(fixture: &Fixture, name: &str, version: &str, extra: &str) {
    write_package(fixture, name, version, extra);
    fs::remove_file(fixture.path().join(format!("packages/{name}/src/lib.rs"))).unwrap();
    fixture.write(&format!("packages/{name}/src/main.rs"), "fn main() {}\n");
}

pub(crate) fn write_package(fixture: &Fixture, name: &str, version: &str, extra: &str) {
    fixture.write(
        &format!("packages/{name}/Cargo.toml"),
        &format!(
            r#"[package]
name = "{name}"
version = "{version}"
edition = "2021"
{extra}
"#
        ),
    );
    fixture.write(&format!("packages/{name}/src/lib.rs"), "pub fn f() {}\n");
}
