//! Hermetic Git helpers for cargo-release-plan integration tests.
//!
//! Git configuration is pinned by `Fixture` so tests do not depend on host or
//! user settings.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use tempfile::TempDir;

use crate::harness::seeded_package;

/// A temporary Git repository that is also a Cargo workspace.
pub(crate) struct Fixture {
    dir: TempDir,
    manifest_path: PathBuf,
}

impl Fixture {
    /// Creates the repository and writes the workspace manifest.
    ///
    /// `extra` is appended to the root manifest, so a caller can add tables such
    /// as `[workspace.dependencies]` or `[workspace.package]`.
    pub(crate) fn new(extra: &str) -> Self {
        let fixture = Self::empty("Cargo.toml");
        fixture.write_workspace(extra);
        fixture
    }

    /// Creates the repository with a workspace manifest at `manifest_path`.
    pub(crate) fn with_workspace_manifest(manifest_path: &str, content: &str) -> Self {
        let fixture = Self::empty(manifest_path);
        if let Some(parent) = fixture.manifest_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&fixture.manifest_path, content).unwrap();
        fixture
    }

    fn empty(manifest_path: &str) -> Self {
        let mut fixture = Self::from_template(&BASE_TEMPLATE);
        fixture.manifest_path = fixture.path().join(manifest_path);
        fixture
    }

    /// Copies an immutable, harness-owned template into an independent repository.
    ///
    /// Copy the objects and index, not hard links or alternates: changing or dropping
    /// one fixture must not affect another. Only ordinary files and directories
    /// belong in templates; tests create symlinks and special index states afterward.
    pub(crate) fn from_template(template: &Self) -> Self {
        let dir = TempDir::new().unwrap();
        copy_directory(template.path(), dir.path());
        let manifest_path = dir.path().join(
            template
                .manifest_path
                .strip_prefix(template.path())
                .unwrap(),
        );
        Self { dir, manifest_path }
    }

    /// Rewrites the root manifest, replacing the tables `new` appended.
    pub(crate) fn write_workspace(&self, extra: &str) {
        // Ordinary supported manifest revisions so `cargo metadata` accepts the
        // generated workspace. Tests do not cover resolver or edition behavior.
        if let Some(parent) = self.manifest_path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(
            &self.manifest_path,
            format!(
                r#"[workspace]
members = ["packages/*"]
resolver = "2"
{extra}
"#
            ),
        )
        .unwrap();
    }

    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    pub(crate) fn manifest(&self) -> PathBuf {
        self.manifest_path.clone()
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

    /// Forces a directory-entry case change without staging the rename in Git.
    pub(crate) fn rename_case(&self, from: &str, to: &str) {
        let from = self.path().join(from);
        // The final spelling can address the source itself; force an actual entry rename.
        let intermediate = from.with_extension("case-rename");
        fs::rename(&from, &intermediate).unwrap();
        fs::rename(intermediate, self.path().join(to)).unwrap();
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
        // These fixtures commit actual work-tree changes. Unlike empty-tree
        // history builders, fast-import would bypass clean filters and fail to
        // preserve the index and mode semantics the integration suite exercises.
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
    }

    /// Runs Cargo against the fixture workspace.
    ///
    /// Offline throughout, since the fixture packages never depend on anything
    /// outside the workspace and a registry lookup would make tests non-hermetic.
    pub(crate) fn cargo(&self, args: &[&str]) -> String {
        let output = Command::new("cargo")
            .current_dir(self.path())
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

/// An initialized, unborn repository reused only as an immutable copy source.
///
/// Git startup is expensive on Windows, so a test process initializes once.
/// The static owner keeps the source alive for every copy. Process-per-test
/// runners only amortize this initialization across fixtures within one test.
static BASE_TEMPLATE: LazyLock<Fixture> = LazyLock::new(|| {
    let dir = TempDir::new().unwrap();
    let manifest_path = dir.path().join("Cargo.toml");
    let fixture = Fixture { dir, manifest_path };
    // An empty template prevents host-installed hooks and sample files from
    // entering the fixture, and keeps the tree copied per test small.
    fixture.git(&["init", "-b", "main", "--template="]);
    let config_path = fixture.path().join(".git/config");
    let config = fs::read_to_string(&config_path).unwrap();
    // These repositories are disposable; durable object flushes and automatic
    // maintenance add cost without protecting any persistent test data. Store
    // the settings locally so Git invoked by the application inherits them too.
    fs::write(
        config_path,
        format!("{config}\n[core]\nfsync = none\n[gc]\nauto = 0\n[maintenance]\nauto = false\n"),
    )
    .unwrap();
    fixture
});

fn copy_directory(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let destination = destination.join(entry.file_name());
        let file_type = entry.file_type().unwrap();
        if file_type.is_dir() {
            copy_directory(&entry.path(), &destination);
        } else {
            assert!(file_type.is_file());
            fs::copy(entry.path(), destination).unwrap();
        }
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
    "maintenance.auto=false",
    "-c",
    "core.fsync=none",
    "-c",
    "core.autocrlf=false",
];

/// A real, empty global configuration shared by commands in this test process.
///
/// Git for Windows on ARM64 rejects the `NUL` device as a configuration path, so
/// commands cannot use a null device. The static owner keeps this immutable file
/// alive without allocating a directory and writing a file for every Git command.
static GLOBAL_CONFIG: LazyLock<TempDir> = LazyLock::new(|| {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("config"), "").unwrap();
    dir
});

/// A `git` command carrying the pinned configuration and no working directory.
///
/// `Fixture::git` runs inside an existing fixture; a test that creates a
/// repository somewhere else, such as a clone, needs the same settings without
/// one.
pub(crate) fn hermetic_git() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", GLOBAL_CONFIG.path().join("config"))
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS");
    command.args(HERMETIC_CONFIG);
    command
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn hermetic_fixtures_preserve_isolation_and_git_semantics() {
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

    let command = hermetic_git();
    let global_config = command
        .get_envs()
        .find(|(name, _)| *name == "GIT_CONFIG_GLOBAL")
        .unwrap()
        .1
        .unwrap();
    assert!(Path::new(global_config).is_file());
    assert!(fs::read(global_config).unwrap().is_empty());
    assert_eq!(
        hermetic_git()
            .get_envs()
            .find(|(name, _)| *name == "GIT_CONFIG_GLOBAL")
            .unwrap()
            .1
            .unwrap(),
        global_config
    );

    let empty = Fixture::new("");
    assert_eq!(
        empty.git(&["symbolic-ref", "HEAD"]).trim(),
        "refs/heads/main"
    );
    assert!(
        hermetic_git()
            .arg("-C")
            .arg(empty.path())
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .unwrap()
            .status
            .code()
            .is_some_and(|code| code != 0)
    );
    assert_eq!(
        empty.git(&["config", "--local", "core.fsync"]).trim(),
        "none"
    );
    assert_eq!(empty.git(&["config", "--local", "gc.auto"]).trim(), "0");
    assert_eq!(
        empty.git(&["config", "--local", "maintenance.auto"]).trim(),
        "false"
    );

    let first = seeded_package();
    let second = seeded_package();
    assert_ne!(first.path(), second.path());
    let seed = second.sha("HEAD");
    assert_eq!(first.sha("HEAD"), seed);
    assert!(second.git(&["status", "--porcelain"]).is_empty());

    // An actual clean filter must still run when the ordinary commit helper
    // stages content. Git itself is available on every supported test host.
    first.git(&["config", "filter.fixture.clean", "git hash-object --stdin"]);
    first.write(".gitattributes", "filtered.txt filter=fixture\n");
    first.write("filtered.txt", "unfiltered fixture bytes\n");
    first.write("packages/demo/src/lib.rs", "pub fn changed() {}\n");
    // Use the index as the mode authority on every host, including filesystems
    // without an executable permission bit. Staging must retain that mode.
    first.git(&["config", "core.fileMode", "false"]);
    first.git(&["update-index", "--chmod=+x", "packages/demo/src/lib.rs"]);
    first.commit("isolated filtered change");
    assert_ne!(first.sha("HEAD"), seed);
    assert_ne!(
        first.git(&["show", "HEAD:filtered.txt"]),
        first.read("filtered.txt")
    );
    assert!(
        first
            .git(&["ls-tree", "HEAD", "packages/demo/src/lib.rs"])
            .starts_with("100755 ")
    );
    drop(first);

    assert_eq!(second.sha("HEAD"), seed);
    assert_eq!(second.read("packages/demo/src/lib.rs"), "pub fn f() {}\n");
    assert!(!second.path().join("filtered.txt").exists());
    assert!(second.git(&["status", "--porcelain"]).is_empty());
    let third = seeded_package();
    assert_eq!(third.sha("HEAD"), seed);
    assert!(third.git(&["status", "--porcelain"]).is_empty());
}

/// Writes a package whose only target is an executable.
///
/// A binary package releases the dependency closure its archive's lockfile
/// records, which a library package does not.
/// Ref: docs/design.md, "Relevant lockfile closures".
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
