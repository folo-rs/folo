//! Disposable fixture repositories and the process helpers every scenario shares.

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use crp_publication::publication::config::Configuration;
use crp_publication::publication::github::PlatformBatch;
use crp_publication::publication::manifest::{Binary, Package, Publication, PublicationManifest};
use crp_publication::publication::packages::PublicationWorkspace;
use serde_json::{Value, json};
use tempfile::TempDir;

/// A disposable Git controller with different source and controller configurations.
pub(crate) struct Fixture {
    pub(crate) root: TempDir,
    pub(crate) source: String,
    pub(crate) triple: String,
    pub(crate) workspace: PathBuf,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("release fixture ")
            .tempdir()
            .unwrap();
        let path = root.path();
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap();
        fs::copy(
            repository.join("rust-toolchain.toml"),
            path.join("rust-toolchain.toml"),
        )
        .unwrap();
        write(
            path,
            "Cargo.toml",
            r#"
[workspace]
members = ["alpha", "beta", "shared"]
resolver = "3"
"#,
        );
        write(
            path,
            ".gitignore",
            "/target\n/out\n/batch.json\n/publication.json\n/cargo-fixture\n/reported-artifact\n",
        );
        write(
            path,
            ".cargo/config.toml",
            "[env]\nRELEASE_FIXTURE_CONFIG = \"tagged\"\n",
        );
        write(
            path,
            "shared/Cargo.toml",
            r#"
[package]
name = "shared"
version = "1.0.0"
edition = "2024"
publish = false
"#,
        );
        write(
            path,
            "shared/src/lib.rs",
            "pub fn message() -> &'static str { env!(\"RELEASE_FIXTURE_CONFIG\") }\n",
        );
        for name in ["alpha", "beta"] {
            write(
                path,
                &format!("{name}/Cargo.toml"),
                &format!(
                    r#"
[package]
name = "{name}"
version = "1.0.0"
edition = "2024"

[[bin]]
name = "{name}-bin"
path = "src/main.rs"

[dependencies]
shared = {{ path = "../shared" }}
"#
                ),
            );
            write(
                path,
                &format!("{name}/src/main.rs"),
                "fn main() { println!(\"{}\", shared::message()); }\n",
            );
            // These are deliberate dummy values, not credentials. The build script enforces
            // that publication credentials are removed from every Cargo subprocess environment.
            write(
                path,
                &format!("{name}/build.rs"),
                r#"
fn main() {
    for name in ["GH_TOKEN", "GITHUB_TOKEN", "GIT_TOKEN", "INPUT_TOKEN", "DEFAULT_GITHUB_TOKEN"] {
        assert!(std::env::var_os(name).is_none(), "credential leaked into a build script");
    }
}
"#,
            );
        }
        run(path, "cargo", &["generate-lockfile", "--offline"]);
        run(path, "git", &["init", "--quiet"]);
        run(
            path,
            "git",
            &["config", "user.email", "fixture@example.invalid"],
        );
        run(path, "git", &["config", "user.name", "Release fixture"]);
        run(path, "git", &["config", "commit.gpgsign", "false"]);
        run(path, "git", &["config", "core.autocrlf", "false"]);
        run(path, "git", &["add", "."]);
        run(path, "git", &["commit", "--quiet", "-m", "Tagged fixture"]);
        let source = run(path, "git", &["rev-parse", "HEAD"]).trim().to_owned();
        write(
            path,
            ".cargo/config.toml",
            "[env]\nRELEASE_FIXTURE_CONFIG = \"controller\"\n",
        );
        // Historical sources need no current controller or publication configuration.
        // The invocation checkout carries the current archive promises for all native targets.
        write(
            path,
            ".cargo/release_plan.toml",
            r#"
schema-version = 1
repository = "fixture/does-not-exist"
release-branch = "main"
targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc", "aarch64-apple-darwin"]
"#,
        );
        for name in ["alpha", "beta"] {
            let manifest = path.join(name).join("Cargo.toml");
            let contents = fs::read_to_string(&manifest).unwrap().replace(
                "edition = \"2024\"",
                "edition = \"2024\"\nrepository = \"https://github.com/fixture/does-not-exist\"",
            );
            fs::write(
                manifest,
                format!(
                    "{contents}\n[package.metadata.binstall]\n\
                     pkg-url = \"{{ repo }}/releases/download/{{ name }}-v{{ version }}/{{ name }}-v{{ version }}-{{ target }}.zip\"\n\
                     bin-dir = \"{{ bin }}{{ binary-ext }}\"\npkg-fmt = \"zip\"\n"
                ),
            )
            .unwrap();
        }
        run(path, "git", &["add", "."]);
        run(
            path,
            "git",
            &["commit", "--quiet", "-m", "Independent controller"],
        );
        let rustc = run(path, "rustc", &["--version", "--verbose"]);
        let triple = rustc
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap()
            .to_owned();
        Self {
            workspace: root.path().to_path_buf(),
            root,
            source,
            triple,
        }
    }

    pub(crate) fn execute(&self, binaries: &Value, output_name: &str) -> Output {
        self.batch_command(binaries, output_name)
            .arg("--no-upload")
            .output()
            .unwrap()
    }

    pub(crate) fn batch_command(&self, binaries: &Value, output_name: &str) -> Command {
        let publication = self.publication();
        let publication_path = self.root.path().join("publication.json");
        publication.write(&publication_path).unwrap();
        let batch = serde_json::from_value::<PlatformBatch>(json!({
            "schema_version": 1, "publication_id": publication.id,
            "repository": "fixture/does-not-exist", "target": self.triple,
            "batch_id": "", "binaries": binaries,
        }))
        .unwrap()
        .seal()
        .unwrap();
        let input = self.root.path().join("batch.json");
        fs::write(&input, serde_json::to_vec(&batch).unwrap()).unwrap();
        let output = self.root.path().join(output_name);
        let mut command = command(self.root.path(), env!("CARGO_BIN_EXE_cargo-release-plan"));
        command
            .args(["publish", "binaries", "--publication"])
            .arg(publication_path)
            .arg("--batch")
            .arg(input)
            .arg("--manifest-path")
            .arg(self.workspace.join("Cargo.toml"))
            .arg("--output")
            .arg(output.join("outcome.json"))
            .arg("--artifacts")
            .arg(output.join("artifacts"))
            .env("CARGO_TARGET_DIR", self.root.path().join("target"))
            .env("CARGO_TERM_COLOR", "never")
            .env_remove("GITHUB_STEP_SUMMARY");
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GIT_TOKEN",
            "INPUT_TOKEN",
            "DEFAULT_GITHUB_TOKEN",
        ] {
            command.env(name, "credential-filter-canary");
        }
        command
    }

    fn publication(&self) -> PublicationManifest {
        let workspace = PublicationWorkspace::load(&self.workspace.join("Cargo.toml")).unwrap();
        let (config_path, configuration) = Configuration::load(workspace.root(), None).unwrap();
        let relative = |path: &Path| {
            path.strip_prefix(self.root.path())
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/")
        };
        let packages = workspace
            .requests(&configuration)
            .unwrap()
            .into_iter()
            .map(|package| Package {
                name: package.name,
                version: package.version,
                manifest: relative(&package.manifest),
                binary: package.binary.map(|binary| Binary {
                    name: binary.name,
                    targets: binary.targets,
                }),
            })
            .collect();
        PublicationManifest::new(Publication {
            schema_version: 1,
            tool_version: env!("CARGO_PKG_VERSION").to_owned(),
            source: run(self.root.path(), "git", &["rev-parse", "HEAD"])
                .trim()
                .to_owned(),
            workspace_manifest: relative(&workspace.root().join("Cargo.toml")),
            config_path: relative(&config_path),
            configuration,
            packages,
        })
        .unwrap()
    }

    pub(crate) fn commit_source(&mut self) {
        run(self.root.path(), "git", &["add", "."]);
        run(
            self.root.path(),
            "git",
            &["commit", "--quiet", "-m", "Source fixture"],
        );
        run(self.root.path(), "git", &["rev-parse", "HEAD"])
            .trim()
            .clone_into(&mut self.source);
    }

    pub(crate) fn binary(&self, name: &str) -> Value {
        json!({
            "name": name, "bin": format!("{name}-bin"), "version": "1.0.0",
            "tag": format!("{name}-v1.0.0"), "source_sha": self.source,
        })
    }

    pub(crate) fn outcomes(&self, output_name: &str) -> Value {
        let outcome: Value = serde_json::from_slice(
            &fs::read(self.root.path().join(output_name).join("outcome.json")).unwrap(),
        )
        .unwrap();
        outcome.get("items").unwrap().clone()
    }
}

// These integration fixtures compile programs and launch toolchain/archive processes;
// allow orders of magnitude more time than their ordinary seconds-long runs.
pub(crate) const SMOKE_WATCHDOG: Duration = Duration::from_mins(5);

pub(crate) fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

pub(crate) fn command(root: &Path, program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root).env_remove("RUSTUP_TOOLCHAIN");
    // Fixture repositories must not inherit workstation hooks, signing or global Git settings.
    command
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1");
    // Applies to nested Git commands as well as fixture setup.
    command
        .env("GIT_CONFIG_COUNT", "4")
        .env("GIT_CONFIG_KEY_0", "user.name")
        .env("GIT_CONFIG_VALUE_0", "Release fixture")
        .env("GIT_CONFIG_KEY_1", "user.email")
        .env("GIT_CONFIG_VALUE_1", "fixture@example.invalid")
        .env("GIT_CONFIG_KEY_2", "commit.gpgsign")
        .env("GIT_CONFIG_VALUE_2", "false")
        .env("GIT_CONFIG_KEY_3", "gc.auto")
        .env("GIT_CONFIG_VALUE_3", "0");
    command
}

pub(crate) fn run(root: &Path, program: impl AsRef<OsStr>, arguments: &[&str]) -> String {
    let result = command(root, program).args(arguments).output().unwrap();
    assert_success(&result);
    String::from_utf8(result.stdout).unwrap()
}

pub(crate) fn compile_tool(root: &Path, name: &str, source: &str) {
    let filename = format!("{name}.rs");
    write(root, &filename, source);
    let result = command(root, "rustc")
        .args([
            "--edition=2024",
            "--crate-name",
            "fixture_tool",
            &filename,
            "-o",
        ])
        .arg(root.join(format!("{name}{EXE_SUFFIX}")))
        .output()
        .unwrap();
    assert_success(&result);
}

pub(crate) fn assert_success(result: &Output) {
    assert!(
        result.status.success(),
        "{}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
