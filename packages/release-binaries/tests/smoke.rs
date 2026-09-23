//! Native no-upload contract: historical sources, shared Cargo output and portable release assets.

#![cfg(not(miri))]
#![cfg_attr(coverage_nightly, feature(coverage_attribute), coverage(off))]
#![allow(
    clippy::indexing_slicing,
    reason = "Fixture JSON has an explicitly asserted shape"
)]

use std::env::consts::EXE_SUFFIX;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;
use std::process::{Command, Output};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

/// A disposable Git controller with different source and controller configurations.
struct Fixture {
    root: TempDir,
    source: String,
    triple: String,
}

// These integration fixtures compile programs and launch toolchain/archive processes;
// allow orders of magnitude more time than their ordinary seconds-long runs.
const SMOKE_WATCHDOG: Duration = Duration::from_mins(5);

impl Fixture {
    fn new() -> Self {
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

    fn execute(&self, binaries: &Value, output_name: &str) -> Output {
        self.batch_command(binaries, output_name).output().unwrap()
    }

    fn batch_command(&self, binaries: &Value, output_name: &str) -> Command {
        let batch = json!({
            "triple": self.triple, "os": "fixture",
            "timeout_minutes": 90_usize.saturating_add(60_usize.saturating_mul(binaries.as_array().unwrap().len())).min(360),
            "binaries": binaries,
        });
        let input = self.root.path().join("batch.json");
        fs::write(&input, serde_json::to_vec(&batch).unwrap()).unwrap();
        let mut command = command(self.root.path(), env!("CARGO_BIN_EXE_release-binaries"));
        command
            .args([
                "run",
                "--repository",
                "fixture/does-not-exist",
                "--no-upload",
                "--input",
            ])
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

    fn commit_source(&mut self) {
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

    fn binary(&self, name: &str) -> Value {
        json!({
            "name": name, "bin": format!("{name}-bin"), "version": "1.0.0",
            "tag": format!("{name}-v1.0.0"), "source_sha": self.source,
        })
    }
}

#[test]
fn stages_tagged_binaries_with_shared_output_and_root_archives() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, stages_tagged_binaries);
}

fn stages_tagged_binaries() {
    let fixture = Fixture::new();
    let result = fixture.execute(
        &json!([fixture.binary("alpha"), fixture.binary("beta")]),
        "out",
    );
    assert_success(&result);
    let outcomes: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("out/outcomes.json")).unwrap())
            .unwrap();
    assert_eq!(outcomes.as_array().unwrap().len(), 2);
    for outcome in outcomes.as_array().unwrap() {
        assert_eq!(outcome["status"], "staged-only");
        assert_eq!(outcome["binary"]["source_sha"], fixture.source);
    }
    for name in ["alpha", "beta"] {
        let base = format!("{name}-v1.0.0-{}", fixture.triple);
        let staging = fixture.root.path().join("out").join(&base);
        let archive = staging.join(format!("{base}.zip"));
        let checksum = fs::read_to_string(staging.join(format!("{base}.sha256"))).unwrap();
        let mut digest = String::new();
        for byte in Sha256::digest(fs::read(&archive).unwrap()) {
            write!(digest, "{byte:02x}").unwrap();
        }
        assert_eq!(
            checksum,
            format!(
                "{digest} {}{base}.zip\n",
                if cfg!(windows) { "*" } else { " " }
            )
        );
        let binary = if cfg!(windows) {
            format!("{name}-bin.exe")
        } else {
            format!("{name}-bin")
        };
        let archive_argument = archive.to_str().unwrap();
        // Inspect through the platform's existing archive implementation, not an in-repo codec.
        write(
            &staging,
            "inspect-archive.ps1",
            "
param([string] $Archive)
$ErrorActionPreference = 'Stop'
$z = [IO.Compression.ZipFile]::OpenRead($Archive)
try {
    ConvertTo-Json -InputObject @($z.Entries | ForEach-Object {
        @{ name = $_.FullName; attributes = $_.ExternalAttributes }
    }) -Compress
} finally { $z.Dispose() }
",
        );
        let inspection = command(&staging, "pwsh")
            .args([
                "-NoProfile",
                "-File",
                "inspect-archive.ps1",
                "-Archive",
                archive_argument,
            ])
            .output()
            .unwrap();
        assert_success(&inspection);
        let entries: Value = serde_json::from_slice(&inspection.stdout).unwrap();
        assert_eq!(entries.as_array().unwrap().len(), 1);
        assert_eq!(entries[0]["name"], binary);
        #[cfg(unix)]
        assert_ne!(
            (entries[0]["attributes"].as_i64().unwrap() >> 16) & 0o111,
            0
        );
        let unpacked = staging.join("unpacked");
        fs::create_dir_all(&unpacked).unwrap();
        #[cfg(windows)]
        run(&unpacked, "7za", &["x", "-y", archive_argument]);
        #[cfg(unix)]
        run(&unpacked, "unzip", &["-q", archive_argument]);
        assert_eq!(run(&unpacked, unpacked.join(&binary), &[]).trim(), "tagged");
    }
    // Both independent builds use the fixture controller's shared target tree.
    assert_eq!(
        String::from_utf8_lossy(&result.stderr)
            .matches("Compiling shared ")
            .count(),
        1
    );
    let deps = fixture
        .root
        .path()
        .join("target")
        .join(&fixture.triple)
        .join("release/deps");
    assert!(fs::read_dir(deps).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("libshared-")
    }));
    assert_eq!(
        run(
            fixture.root.path(),
            "git",
            &["worktree", "list", "--porcelain"]
        )
        .matches("worktree ")
        .count(),
        1
    );
    assert!(run(fixture.root.path(), "git", &["status", "--porcelain"]).is_empty());
}

#[test]
fn failed_item_does_not_publish_or_prevent_independent_staging() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, stages_after_item_failure);
}

fn stages_after_item_failure() {
    let fixture = Fixture::new();
    let mut invalid = fixture.binary("alpha");
    invalid["version"] = "9.0.0".into();
    invalid["tag"] = "alpha-v9.0.0".into();
    let result = fixture.execute(&json!([invalid, fixture.binary("beta")]), "out");
    assert!(!result.status.success());
    let outcomes: Value =
        serde_json::from_slice(&fs::read(fixture.root.path().join("out/outcomes.json")).unwrap())
            .unwrap();
    assert_eq!(outcomes[0]["status"], "failed");
    assert_eq!(outcomes[0]["stage"], "build");
    assert_eq!(outcomes[1]["status"], "staged-only");
    assert_eq!(
        run(
            fixture.root.path(),
            "git",
            &["worktree", "list", "--porcelain"]
        )
        .matches("worktree ")
        .count(),
        1
    );
}

#[test]
fn package_features_do_not_leak_across_separate_builds() {
    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        write(
            fixture.root.path(),
            "shared/Cargo.toml",
            r#"
[package]
name = "shared"
version = "1.0.0"
edition = "2024"
[features]
extra = []
"#,
        );
        write(
            fixture.root.path(),
            "shared/src/lib.rs",
            "pub fn message() -> &'static str { if cfg!(feature = \"extra\") { \"extra\" } else { \"base\" } }\n",
        );
        let manifest = fixture.root.path().join("alpha/Cargo.toml");
        let contents = fs::read_to_string(&manifest).unwrap().replace(
            "shared = { path = \"../shared\" }",
            "shared = { path = \"../shared\", features = [\"extra\"] }",
        );
        fs::write(manifest, contents).unwrap();
        fixture.commit_source();
        let result = fixture.execute(
            &json!([fixture.binary("alpha"), fixture.binary("beta")]),
            "out",
        );
        assert_success(&result);
        for (name, expected) in [("alpha", "extra"), ("beta", "base")] {
            let base = format!("{name}-v1.0.0-{}", fixture.triple);
            let filename = format!("{name}-bin{EXE_SUFFIX}");
            let staged = fixture.root.path().join("out").join(base);
            assert_eq!(run(&staged, staged.join(filename), &[]).trim(), expected);
        }
    });
}

#[cfg(unix)]
#[test]
fn cancellation_terminates_the_build_tree_and_cleans_source() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::Stdio;

    testing::with_watchdog_timeout(SMOKE_WATCHDOG, || {
        let mut fixture = Fixture::new();
        // The socket is an explicit readiness/termination handshake, never a time-based wait.
        // Holding it open inside the build script proves cancellation reaches Cargo descendants.
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        write(
            fixture.root.path(),
            "alpha/build.rs",
            r#"
use std::io::{Read, Write};
fn main() {
    let mut socket = std::net::TcpStream::connect(std::env::var("RELEASE_FIXTURE_SOCKET").unwrap()).unwrap();
    socket.write_all(b"ready").unwrap();
    let mut request = [0];
    socket.read_exact(&mut request).unwrap();
    socket.write_all(b"waiting").unwrap();
    socket.read_exact(&mut request).unwrap();
}
"#,
        );
        fixture.commit_source();
        let mut process = fixture
            .batch_command(
                &json!([fixture.binary("alpha"), fixture.binary("beta")]),
                "out",
            )
            .env(
                "RELEASE_FIXTURE_SOCKET",
                listener.local_addr().unwrap().to_string(),
            )
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let (mut socket, _) = listener.accept().unwrap();
        let mut ready = [0; 5];
        socket.read_exact(&mut ready).unwrap();
        assert_eq!(&ready, b"ready");
        socket.write_all(b"x").unwrap();
        let mut waiting = [0; 7];
        socket.read_exact(&mut waiting).unwrap();
        assert_eq!(&waiting, b"waiting");
        run(
            fixture.root.path(),
            "kill",
            &["-TERM", &process.id().to_string()],
        );
        assert!(!process.wait().unwrap().success());
        // No surviving build script retains its socket after the helper reports failure.
        assert_eq!(socket.read(&mut ready).unwrap(), 0);
        let outcomes: Value = serde_json::from_slice(
            &fs::read(fixture.root.path().join("out/outcomes.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(outcomes[0]["status"], "failed");
        assert_eq!(outcomes[1]["status"], "unattempted");
        assert!(outcomes[0]["cleanup_error"].is_null());
        assert_eq!(
            run(
                fixture.root.path(),
                "git",
                &["worktree", "list", "--porcelain"]
            )
            .matches("worktree ")
            .count(),
            1
        );
    });
}

fn write(root: &Path, relative: &str, content: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

fn command(root: &Path, program: impl AsRef<OsStr>) -> Command {
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

fn run(root: &Path, program: impl AsRef<OsStr>, arguments: &[&str]) -> String {
    let result = command(root, program).args(arguments).output().unwrap();
    assert_success(&result);
    String::from_utf8(result.stdout).unwrap()
}

fn assert_success(result: &Output) {
    assert!(
        result.status.success(),
        "{}\n{}\n{}",
        result.status,
        String::from_utf8_lossy(&result.stdout),
        String::from_utf8_lossy(&result.stderr)
    );
}
