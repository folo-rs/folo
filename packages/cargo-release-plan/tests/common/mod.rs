//! Hermetic Git helpers for cargo-release-plan integration tests.
//!
//! Every `git` invocation pins identity and throughput config so tests do not
//! depend on the host or user configuration.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// A temporary Git repository that is also a Cargo workspace.
pub(crate) struct Fixture {
    dir: TempDir,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let fixture = Self { dir };
        fixture.git(&["init", "-b", "main"]);
        fixture
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

    pub(crate) fn git(&self, args: &[&str]) -> String {
        let mut command = Command::new("git");
        command.args([
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
            "-C",
        ]);
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

    pub(crate) fn sha(&self, rev: &str) -> String {
        self.git(&["rev-parse", rev]).trim().to_string()
    }
}

pub(crate) fn write_workspace(fixture: &Fixture, extra: &str) {
    fixture.write(
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
