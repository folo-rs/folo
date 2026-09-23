//! Disposable fixture repositories and the process helpers every scenario shares.

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};
use tempfile::TempDir;

/// A disposable Git controller with different source and controller configurations.
pub(crate) struct Fixture {
    pub(crate) root: TempDir,
    pub(crate) source: String,
    pub(crate) triple: String,
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
        for relative in [
            "rust-toolchain.toml",
            "scripts/release/Install-ReleaseSourceToolchain.ps1",
            "scripts/setup/RustToolchain.psm1",
            "scripts/utility/Retry.psm1",
        ] {
            let destination = path.join(relative);
            fs::create_dir_all(destination.parent().unwrap()).unwrap();
            fs::copy(repository.join(relative), destination).unwrap();
        }
        write(
            path,
            "Cargo.toml",
            r#"
[workspace]
members = ["alpha", "beta", "shared"]
resolver = "3"
"#,
        );
        write(path, ".gitignore", "/target\n/out\n/batch.json\n");
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
        let batch = json!({
            "triple": self.triple, "os": "fixture",
            "timeout_minutes": 90_usize.saturating_add(60_usize.saturating_mul(binaries.as_array().unwrap().len())).min(360),
            "binaries": binaries,
        });
        let input = self.root.path().join("batch.json");
        fs::write(&input, serde_json::to_vec(&batch).unwrap()).unwrap();
        let mut command = command(self.root.path(), env!("CARGO_BIN_EXE_release-binaries"));
        command
            .args(["run", "--repository", "fixture/does-not-exist", "--input"])
            .arg(input)
            .arg("--controller")
            .arg(self.root.path())
            .arg("--output")
            .arg(self.root.path().join(output_name))
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
