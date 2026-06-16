//! End-to-end integration tests for the Azure Blob storage backend.
//!
//! These drive the public `run` and `analyze` commands against a live Azurite
//! emulator, proving the Azure adapter stores result sets (`run`) and reads them
//! back (`analyze`) through exactly the same command surface the binary uses.
//!
//! They compile only with the `azure` feature and require an Azurite blob
//! endpoint (the CI `azure` job provides one; see the package AGENTS.md for
//! running them locally). Each test uses a fresh container, so they never share
//! state, and they are ignored under Miri (real network and process I/O).
#![cfg(feature = "azure")]
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

use std::net::{TcpStream, ToSocketAddrs as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use argh::FromArgs;
use azure_core::http::Url;
use cargo_bench_history::{Cli, Command, RunError, RunOutcome, run_with_target_root};
use serial_test::serial;

#[path = "support/cwd_guard.rs"]
mod cwd_guard;
use cwd_guard::CwdGuard;

/// The mock engine binary path, provided by Cargo for the `[[bin]]` target.
const MOCK_ENGINE: &str = env!("CARGO_BIN_EXE_cargo-bench-history-mock-engine");

/// The well-known Azurite development account key (public, fixed, not secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .expect("arguments should parse")
        .into_command()
}

/// The Azurite blob endpoint, overridable for a non-default emulator.
fn azurite_endpoint() -> String {
    std::env::var("AZURITE_BLOB_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".to_owned())
}

/// Whether an Azurite blob endpoint is reachable via a short TCP connect.
///
/// The `azure` feature builds these tests under `--all-features`, where the
/// runner usually has no emulator. A reachability probe lets each test self-skip
/// there while still running for real wherever Azurite is provided.
///
/// Setting `BENCH_HISTORY_REQUIRE_AZURITE` turns an unreachable emulator into a
/// hard failure, so the dedicated CI job that provisions Azurite cannot silently
/// degrade into skipping every network test.
fn azurite_available() -> bool {
    let endpoint = azurite_endpoint();
    let reachable = Url::parse(&endpoint)
        .ok()
        .and_then(|url| {
            let host = url.host_str().unwrap_or("127.0.0.1").to_owned();
            let port = url.port().unwrap_or(10000);
            (host.as_str(), port).to_socket_addrs().ok()
        })
        .into_iter()
        .flatten()
        .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok());

    if !reachable {
        assert!(
            std::env::var_os("BENCH_HISTORY_REQUIRE_AZURITE").is_none(),
            "BENCH_HISTORY_REQUIRE_AZURITE is set but no Azurite emulator is reachable at {endpoint}"
        );
        eprintln!("skipping Azurite integration test: no emulator reachable at {endpoint}");
    }
    reachable
}

/// A fresh, valid container name (lowercase, 3-63 chars) unique to one test.
fn unique_container() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = jiff::Timestamp::now().as_nanosecond();
    format!("bh-it-{nanos}-{n}")
}

/// Builds a config `command` line that invokes the mock engine with `args`,
/// POSIX single-quoting the path so it survives shell-word tokenization.
fn mock_command(args: &str) -> String {
    let quoted = format!("'{}'", MOCK_ENGINE.replace('\'', r"'\''"));
    if args.is_empty() {
        quoted
    } else {
        format!("{quoted} {args}")
    }
}

/// Escapes a string for embedding in a TOML basic (double-quoted) string.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A config that stores in a fresh Azurite container and runs the mock engine.
fn azure_config(command: &str) -> String {
    format!(
        "[project]\n\
         id = \"azureproj\"\n\n\
         [storage.azure]\n\
         account = \"devstoreaccount1\"\n\
         container = \"{container}\"\n\
         endpoint = \"{endpoint}\"\n\
         account_key = \"{key}\"\n\n\
         [engines.callgrind]\n\
         command = \"{command}\"\n",
        container = unique_container(),
        endpoint = toml_escape(&azurite_endpoint()),
        key = AZURITE_KEY,
        command = toml_escape(command),
    )
}

/// A hermetic workspace that stores to Azurite rather than the local filesystem.
///
/// It is a clean git repository: `analyze` resolves a series' timeline from git
/// topology, so it requires a repository, and the directories a run touches
/// (`.cargo`, `target`) are git-ignored so the real probe records clean (not
/// dirty) runs against the committed code.
struct AzureWorkspace {
    dir: tempfile::TempDir,
}

impl AzureWorkspace {
    fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().expect("temp dir should be created"),
        };
        let cargo_dir = workspace.dir.path().join(".cargo");
        std::fs::create_dir_all(&cargo_dir).unwrap();
        std::fs::write(cargo_dir.join("bench_history.toml"), config).unwrap();
        std::fs::write(
            workspace.dir.path().join(".gitignore"),
            "/.cargo/\n/target/\n",
        )
        .unwrap();
        workspace.git(&["init", "-b", "master"]);
        workspace.git(&["config", "user.email", "test@example.invalid"]);
        workspace.git(&["config", "user.name", "Bench History Test"]);
        workspace.git(&["config", "commit.gpgsign", "false"]);
        workspace.git(&["add", ".gitignore"]);
        workspace.git(&["commit", "-m", "root"]);
        workspace
    }

    /// Runs `git -C <root> <args>`, asserting success.
    fn git(&self, args: &[&str]) -> std::process::Output {
        let root = self.dir.path().to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec!["-C", root.as_str()];
        full.extend_from_slice(args);
        let output = std::process::Command::new("git")
            .args(&full)
            .output()
            .expect("git should be available");
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    }

    /// Creates an empty commit so a subsequent clean run lands on a fresh commit
    /// directory (a clean run is keyed solely by its commit, so each point in a
    /// history needs its own commit).
    fn commit(&self, message: &str) {
        self.git(&["commit", "--allow-empty", "-m", message]);
    }

    /// Drives a command with `args` from inside this workspace, pointing the
    /// harvest at the workspace's own `target/` so it is hermetic.
    async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        let _cwd = CwdGuard::enter(self.dir.path());

        let target_root = self.dir.path().join("target");
        run_with_target_root(&command_from(args), Some(target_root)).await
    }
}

/// `run` stores a harvested result set in Azure Blob storage.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_stores_results_in_azurite() {
    if !azurite_available() {
        return;
    }
    let workspace = AzureWorkspace::new(&azure_config(&mock_command("--summary grp=single")));

    let outcome = workspace
        .drive(&["run", "--timestamp", "2024-01-01T00:00:00Z"])
        .await
        .expect("run should store to Azurite");
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");
}

/// A full public round-trip: two `run`s store result sets to Azurite, and
/// `analyze` reads them back (list + get) and reports over the history.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[serial]
async fn run_then_analyze_round_trips_through_azurite() {
    if !azurite_available() {
        return;
    }
    let workspace = AzureWorkspace::new(&azure_config(&mock_command("--summary grp=single")));

    workspace
        .drive(&["run", "--timestamp", "2024-01-01T00:00:00Z"])
        .await
        .expect("first run should store to Azurite");
    // A clean run is keyed by its commit, so the second point needs its own
    // commit; otherwise it would collide with the first on the same clean key.
    workspace.commit("second");
    workspace
        .drive(&["run", "--timestamp", "2024-01-02T00:00:00Z"])
        .await
        .expect("second run should store to Azurite");

    let outcome = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .expect("analyze should read the history back from Azurite");
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };

    // The report parsing proves `analyze` listed and fetched both stored objects.
    let parsed: serde_json::Value =
        serde_json::from_str(&report).expect("the json report should parse");
    assert_eq!(parsed["project"], "azureproj");
    // A flat two-point series is not a regression.
    assert_eq!(regressions, 0);
    assert_eq!(parsed["regressions"], 0);
}
