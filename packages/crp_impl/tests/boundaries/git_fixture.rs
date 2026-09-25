//! Repository fixtures for integration tests of Git and discovery boundaries.
//!
//! Unlike CLI integration fixtures, these acquire no Cargo metadata or resolution.

use std::fs;
use std::path::Path;
use std::process::Command;

use crp_impl::git::GitRepo;
use tempfile::{TempDir, tempdir};

/// An independently mutable repository for exercising one acquisition boundary.
pub(crate) struct Repository {
    directory: TempDir,
}

impl Repository {
    pub(crate) fn new() -> Self {
        let fixture = Self {
            directory: tempdir().unwrap(),
        };
        fs::write(fixture.path().join("global-config"), "").unwrap();
        fixture.command(&["init", "--quiet", "--template="]);
        // Index modes are authoritative on every test host. Tests of actual
        // work-tree executable bits remain in the Git and integration suites.
        fixture.command(&["config", "core.fileMode", "false"]);
        // GitRepo uses normal subprocess configuration. Local settings keep its
        // reads consistent with staging even when the host has global ignore,
        // attribute or line-ending rules.
        for key in ["core.excludesFile", "core.attributesFile"] {
            fixture.command(&[
                "config",
                key,
                &fixture.path().join("global-config").to_string_lossy(),
            ]);
        }
        fixture.command(&["config", "core.autocrlf", "false"]);
        fixture.command(&["config", "core.eol", "lf"]);
        // Disposable fixtures must not start a daemon from inherited host settings.
        fixture.command(&["config", "core.fsmonitor", "false"]);
        fixture
    }

    pub(crate) fn path(&self) -> &Path {
        self.directory.path()
    }

    pub(crate) fn repo(&self) -> GitRepo {
        GitRepo {
            root: self.path().to_path_buf(),
            prefix: String::new(),
        }
    }

    pub(crate) fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    pub(crate) fn command(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(self.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", self.path().join("global-config"))
            .env_remove("GIT_CONFIG")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .args([
                "-c",
                "user.email=release-plan@example.invalid",
                "-c",
                "user.name=Release Plan Test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "gc.auto=0",
                "-c",
                "maintenance.auto=false",
                "-c",
                "core.autocrlf=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout).unwrap()
    }
}
