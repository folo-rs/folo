//! End-to-end integration tests for the Azure Blob storage backend.
//!
//! These drive the public `run` and `analyze` commands against a live Azure Blob
//! endpoint, proving the Azure adapter stores result sets (`run`) and reads them
//! back (`analyze`) through exactly the same command surface the binary uses.
//!
//! There are two flavours of the same scenarios:
//!
//! * **Azurite** (`*_in_azurite`) — against a local Azurite emulator using the
//!   self-signed account-SAS path. They **self-skip** when no emulator is reachable
//!   (so a normal test run stays green) and run for real once Azurite is up; CI
//!   provides one in the `test-azurite` job.
//! * **Real Azure** (`*_in_real_azure`) — against a real Storage account using the
//!   **Microsoft Entra ID** path (no account key). They self-skip unless
//!   `ENABLE_AZURE` is set (an explicit opt-in, so the account name living in
//!   `constants.env` does not by itself make a plain test run target the cloud);
//!   CI provides one in the `test-azure` job, signing in via GitHub OIDC workload
//!   identity federation, and locally `just test-azure` sets it after `az login`
//!   (see the package AGENTS.md and `infra/azure-bench-history-test/`). The account name
//!   comes from `BENCH_HISTORY_TEST_AZURE_ACCOUNT`. Each test uses a fresh container
//!   that is deleted when the test finishes, even on panic.
//!
//! Each scenario uses its own container so they never share state, and they are
//! ignored under Miri (real network and process I/O).
#![allow(clippy::indexing_slicing, reason = "panic is fine in tests")]

use std::net::{TcpStream, ToSocketAddrs as _};
use std::panic;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use azure_core::credentials::TokenCredential;
use azure_core::http::Url;
use azure_identity::DeveloperToolsCredential;
use azure_storage_blob::BlobContainerClient;
use cargo_bench_history::{Cli, Command, Overrides, RunError, RunOutcome, run_with_overrides};
use futures::FutureExt as _;
use serial_test::serial;

/// The mock engine binary path, provided by Cargo for the auto-discovered binary target.
const MOCK_ENGINE: &str = env!("CARGO_BIN_EXE_cargo-bench-history-mock-engine");

/// The well-known Azurite development account key (public, fixed, not secret).
const AZURITE_KEY: &str =
    "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

fn command_from(args: &[&str]) -> Command {
    Cli::from_args(&["cargo-bench-history"], args)
        .unwrap()
        .into_command()
}

/// The Azurite blob endpoint, overridable for a non-default emulator.
fn azurite_endpoint() -> String {
    std::env::var("AZURITE_BLOB_ENDPOINT")
        .unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".to_owned())
}

/// Whether an Azurite blob endpoint is reachable via a short TCP connect.
///
/// These tests are always compiled, but the runner usually has no emulator. A
/// reachability probe lets each test self-skip there while still running for real
/// wherever Azurite is provided.
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

/// Escapes a string for embedding in a TOML basic (double-quoted) string.
fn toml_escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A config that stores in a fresh Azurite container.
fn azure_config() -> String {
    azure_config_for(&unique_container())
}

/// A config that stores in the named Azurite container, so a test can inspect the
/// same container directly afterward.
fn azure_config_for(container: &str) -> String {
    format!(
        "[project]\n\
         id = \"azureproj\"\n\n\
         [storage.azure]\n\
         account = \"devstoreaccount1\"\n\
         container = \"{container}\"\n\
         endpoint = \"{endpoint}\"\n\
         account_key = \"{key}\"\n",
        endpoint = toml_escape(&azurite_endpoint()),
        key = AZURITE_KEY,
    )
}

/// Mints an account SAS query for the Azurite dev account, mirroring the
/// production minting in `storage::sas` (which is unit-tested there). It is
/// reproduced here because the Azure SDK exposes no shared-key credential and the
/// production minting is crate-private, so a test that inspects blobs directly
/// must sign its own token.
fn azurite_account_sas() -> String {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use hmac::{Hmac, KeyInit as _, Mac as _};
    use sha2::Sha256;

    let expiry = "2030-01-01T00:00:00Z";
    let protocol = "https,http";
    let string_to_sign =
        format!("devstoreaccount1\nrwdlac\nb\nsco\n\n{expiry}\n\n{protocol}\n2021-08-06\n\n");
    let key = BASE64.decode(AZURITE_KEY).unwrap();
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).unwrap();
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64.encode(mac.finalize().into_bytes());

    let mut url = Url::parse("http://sas.invalid/").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("sv", "2021-08-06"),
        ("ss", "b"),
        ("srt", "sco"),
        ("sp", "rwdlac"),
        ("se", expiry),
        ("spr", protocol),
        ("sig", signature.as_str()),
    ]);
    url.query().unwrap().to_owned()
}

/// A SAS-authenticated container client for inspecting blobs Azurite stored,
/// bypassing the production backend (which would transparently inflate them).
fn azurite_container_client(container: &str) -> BlobContainerClient {
    let mut url = Url::parse(&azurite_endpoint()).unwrap();
    url.path_segments_mut()
        .unwrap()
        .pop_if_empty()
        .push(container);
    url.set_query(Some(&azurite_account_sas()));
    BlobContainerClient::new(url, None, None).unwrap()
}

/// The real Storage account name to target, or `None` when none is configured.
fn real_azure_account() -> Option<String> {
    std::env::var("BENCH_HISTORY_TEST_AZURE_ACCOUNT")
        .ok()
        .filter(|account| !account.is_empty())
}

/// The blob endpoint for `account`, overridable via `BENCH_HISTORY_AZURE_ENDPOINT`.
fn real_azure_endpoint(account: &str) -> String {
    std::env::var("BENCH_HISTORY_AZURE_ENDPOINT")
        .unwrap_or_else(|_| format!("https://{account}.blob.core.windows.net"))
}

/// Whether the real-Azure tests are enabled.
///
/// They run only when `ENABLE_AZURE` is set, so a plain test run self-skips them
/// even though `BENCH_HISTORY_TEST_AZURE_ACCOUNT` may be in scope. `ENABLE_AZURE` is an
/// explicit opt-in — set by the `just test-azure` recipe and the CI `test-azure`
/// job — that says "really run these", so a then-missing
/// `BENCH_HISTORY_TEST_AZURE_ACCOUNT` is a hard failure rather than a silent skip,
/// mirroring how `BENCH_HISTORY_REQUIRE_AZURITE` guards Azurite.
fn real_azure_enabled() -> bool {
    if std::env::var_os("ENABLE_AZURE").is_none_or(|value| value.is_empty()) {
        eprintln!("skipping real Azure integration test: ENABLE_AZURE not set");
        return false;
    }
    assert!(
        real_azure_account().is_some(),
        "ENABLE_AZURE is set but BENCH_HISTORY_TEST_AZURE_ACCOUNT names no account"
    );
    true
}

/// A config that stores in `container` on the real account via Microsoft Entra ID.
///
/// It sets neither `account_key` nor `sas_token`, so `AzureBlobStorage` resolves the
/// Entra credential path — the production default, which Azurite never exercises.
fn real_azure_config(container: &str) -> String {
    let account = real_azure_account().expect("real Azure account is configured");
    format!(
        "[project]\n\
         id = \"azureproj\"\n\n\
         [storage.azure]\n\
         account = \"{account}\"\n\
         container = \"{container}\"\n\
         endpoint = \"{endpoint}\"\n",
        endpoint = toml_escape(&real_azure_endpoint(&account)),
    )
}

/// Runs `body` against a fresh real-Azure container, deleting the container
/// afterward even if `body` panics, so a failed test never leaks storage.
async fn with_real_azure_container<F>(body: F)
where
    F: AsyncFnOnce(String),
{
    let account = real_azure_account().expect("real Azure account is configured");
    let endpoint = real_azure_endpoint(&account);
    let container = unique_container();

    // Capture a panic so the container is always deleted, then re-raise it.
    let result = panic::AssertUnwindSafe(body(container.clone()))
        .catch_unwind()
        .await;

    delete_container(&endpoint, &container).await;

    if let Err(payload) = result {
        panic::resume_unwind(payload);
    }
}

/// Deletes `container` at `endpoint` using the Entra credential, best-effort.
///
/// Every fallible step logs a warning and returns instead of panicking. This runs
/// during cleanup between `catch_unwind` and `resume_unwind`, so a panic here would
/// both leak the container and mask the original test failure being re-raised.
async fn delete_container(endpoint: &str, container: &str) {
    let credential: Arc<dyn TokenCredential> = match DeveloperToolsCredential::new(None) {
        Ok(credential) => credential,
        Err(error) => {
            eprintln!(
                "warning: could not initialize Entra credential to delete test container \
                 {container}: {error}"
            );
            return;
        }
    };
    let mut url = match Url::parse(endpoint) {
        Ok(url) => url,
        Err(error) => {
            eprintln!(
                "warning: could not parse endpoint {endpoint} to delete test container \
                 {container}: {error}"
            );
            return;
        }
    };
    {
        let Ok(mut segments) = url.path_segments_mut() else {
            eprintln!(
                "warning: endpoint {endpoint} is not a base URL; cannot delete test container \
                 {container}"
            );
            return;
        };
        segments.pop_if_empty().push(container);
    }
    let client = match BlobContainerClient::new(url, Some(credential), None) {
        Ok(client) => client,
        Err(error) => {
            eprintln!(
                "warning: could not build container client to delete test container \
                 {container}: {error}"
            );
            return;
        }
    };
    if let Err(error) = client.delete(None).await {
        eprintln!("warning: could not delete test container {container}: {error}");
    }
}

/// A hermetic workspace that stores to Azurite rather than the local filesystem.
///
/// It is a clean git repository: `analyze` resolves a series' timeline from git
/// topology, so it requires a repository, and the directories a run touches
/// (`.cargo`, `target`) are git-ignored so the real probe records clean (not
/// dirty) runs against the committed code.
struct AzureWorkspace {
    dir: tempfile::TempDir,
    bench: Vec<String>,
}

impl AzureWorkspace {
    fn new(config: &str) -> Self {
        let workspace = Self {
            dir: tempfile::tempdir().unwrap(),
            bench: Vec::new(),
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
        workspace.git(&["add", ".gitignore"]);
        workspace.git(&["commit", "-m", "root"]);
        workspace
    }

    /// Sets the arguments the mock benchmark engine receives, describing the
    /// fixtures it should emit. A run invokes the mock once with these arguments
    /// and harvests every engine output tree it wrote.
    fn with_bench(mut self, args: &[&str]) -> Self {
        self.bench = args.iter().map(|arg| (*arg).to_owned()).collect();
        self
    }

    /// Runs `git -C <root> <args>`, asserting success. Each invocation injects the
    /// committer identity plus `core.fsync=none`/`gc.auto=0` so commits skip the
    /// per-object disk flush (the dominant per-commit cost on Windows) and no
    /// background repack fires; this keeps a throwaway repository to `init`/`add`/
    /// `commit` rather than several extra `git config` spawns.
    fn git(&self, args: &[&str]) -> std::process::Output {
        let root = self.dir.path().to_string_lossy().into_owned();
        let mut full: Vec<&str> = vec![
            "-c",
            "user.email=test@example.invalid",
            "-c",
            "user.name=Bench History Test",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.fsync=none",
            "-c",
            "core.fsyncObjectFiles=false",
            "-c",
            "gc.auto=0",
            "-C",
            root.as_str(),
        ];
        full.extend_from_slice(args);
        let output = std::process::Command::new("git")
            .args(&full)
            .output()
            .unwrap();
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

    /// Creates and checks out a new branch off the current `HEAD`.
    fn checkout_new_branch(&self, name: &str) {
        self.git(&["checkout", "-b", name]);
    }

    /// Writes an untracked (and not git-ignored) file, leaving the working tree
    /// dirty so the next run records a dirty snapshot.
    fn make_dirty(&self, relative: &str) {
        std::fs::write(self.dir.path().join(relative), "uncommitted\n").unwrap();
    }

    /// Drives a command with `args` against this workspace, pointing the
    /// harvest at the workspace's own `target/` so it is hermetic.
    async fn drive(&self, args: &[&str]) -> Result<RunOutcome, RunError> {
        let target_root = self.dir.path().join("target");
        // Drive `run`/`backfill` against the mock engine instead of `cargo bench`:
        // the program plus its fixture-describing arguments form the benchmark
        // command, which the single bench invocation runs to produce engine output.
        let mut bench_command = vec![MOCK_ENGINE.to_owned()];
        bench_command.extend(self.bench.iter().cloned());
        run_with_overrides(
            &command_from(args),
            Overrides {
                workspace_dir: Some(self.dir.path().to_path_buf()),
                target_root: Some(target_root),
                bench_command: Some(bench_command),
                now: None,
            },
        )
        .await
    }
}

/// Scenario: a single `run` stores one harvested result set.
async fn scenario_run_stores(config: &str) {
    let workspace = AzureWorkspace::new(config).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");
}

/// Scenario: a full public round-trip — two `run`s store result sets, and
/// `analyze` reads them back (list + get) and reports over the history.
async fn scenario_run_then_analyze(config: &str) {
    let workspace = AzureWorkspace::new(config).with_bench(&["--summary", "grp=single"]);

    workspace.drive(&["run"]).await.unwrap();
    // A clean run is keyed by its commit, so the second point needs its own
    // commit; otherwise it would collide with the first on the same clean key.
    workspace.commit("second");
    workspace.drive(&["run"]).await.unwrap();

    let outcome = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap();
    let RunOutcome::Analyzed {
        report,
        regressions,
        ..
    } = outcome
    else {
        panic!("expected an analyzed outcome, got {outcome:?}");
    };

    // The report parsing proves `analyze` listed and fetched both stored objects.
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(parsed["project"], "azureproj");
    // A flat two-point series is not a regression.
    assert_eq!(regressions, 0);
    assert_eq!(parsed["regressions"], 0);
}

/// Scenario: a non-trivial round-trip — a multi-commit master line plus a feature
/// branch carrying a clean and a dirty snapshot. This proves `list(prefix)`
/// enumerates objects across several commit partitions and that the git-aware
/// feature/official dirty-admission split works end to end against the backend (not
/// just a flat two-object listing).
async fn scenario_feature_and_dirty(config: &str) {
    let workspace = AzureWorkspace::new(config).with_bench(&["--summary", "grp=single"]);

    // master: root - c2   (two clean points on the official line).
    workspace.drive(&["run"]).await.unwrap();
    workspace.commit("c2");
    workspace.drive(&["run"]).await.unwrap();

    // feature off c2: one clean point plus a dirty snapshot on the same commit.
    workspace.checkout_new_branch("feature");
    workspace.commit("f1");
    workspace.drive(&["run"]).await.unwrap();
    workspace.make_dirty("uncommitted.txt");
    workspace.drive(&["run"]).await.unwrap();

    // The feature view admits the dirty snapshot on the target-side commit, so all
    // four stored objects are loaded from the backend.
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&["analyze", "--format", "json"])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["runs"], 4,
        "the feature view loads both master points plus the clean and dirty feature \
         snapshots: {report}"
    );

    // The official view (master) admits only the two clean master points: the
    // feature commit is off master's first-parent line and the dirty snapshot is
    // excluded regardless. Master's recorded data set is all-clean (the dirty run
    // sits on the feature branch, off master's line), so mode auto-detection picks
    // `history` even though the working tree is currently dirty — the decision keys
    // off the data, not the on-disk tree. `--since 2020-01-01` is a generous lower
    // bound that keeps the freshly-committed runs inside the window history mode
    // would otherwise default to (six months back).
    let RunOutcome::Analyzed { report, .. } = workspace
        .drive(&[
            "analyze",
            "--context",
            "master",
            "--since",
            "2020-01-01",
            "--format",
            "json",
        ])
        .await
        .unwrap()
    else {
        panic!("expected an analyzed outcome");
    };
    let parsed: serde_json::Value = serde_json::from_str(&report).unwrap();
    assert_eq!(
        parsed["mode"], "history",
        "an all-clean master data set is the history view despite the dirty tree: {report}"
    );
    assert_eq!(
        parsed["runs"], 2,
        "the official line excludes the feature commit and the dirty snapshot: {report}"
    );
}

// --- Azurite (self-signed account SAS) -------------------------------------

/// `run` stores a harvested result set in Azurite.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Azure network end-to-end test: self-skips without an emulator (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn run_stores_results_in_azurite() {
    if !azurite_available() {
        return;
    }
    scenario_run_stores(&azure_config()).await;
}

/// A `run` + `analyze` round-trip through Azurite.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Azure network end-to-end test: self-skips without an emulator (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn run_then_analyze_round_trips_through_azurite() {
    if !azurite_available() {
        return;
    }
    scenario_run_then_analyze(&azure_config()).await;
}

/// A multi-commit feature/dirty round-trip through Azurite.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Azure network end-to-end test: self-skips without an emulator (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn analyze_feature_and_dirty_round_trip_through_azurite() {
    if !azurite_available() {
        return;
    }
    scenario_feature_and_dirty(&azure_config()).await;
}

/// A stored blob carries `Content-Encoding: gzip`, so a non-SDK reader knows the
/// body is compressed. The production round-trip tests cannot prove this — the
/// backend inflates on `get` regardless of the header — so this inspects the blob
/// directly through a SAS-authenticated client.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Azure network end-to-end test: self-skips without an emulator (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn stored_blob_declares_gzip_content_encoding_in_azurite() {
    use azure_storage_blob::models::BlobClientGetPropertiesResultHeaders as _;
    use futures::TryStreamExt as _;

    if !azurite_available() {
        return;
    }

    let container = unique_container();
    let workspace =
        AzureWorkspace::new(&azure_config_for(&container)).with_bench(&["--summary", "grp=single"]);
    workspace.drive(&["run"]).await.unwrap();

    let client = azurite_container_client(&container);
    let mut pager = client.list_blobs(None).unwrap();
    let mut names = Vec::new();
    while let Some(item) = pager.try_next().await.unwrap() {
        if let Some(name) = item.name {
            names.push(name);
        }
    }
    assert_eq!(
        names.len(),
        1,
        "the run stored exactly one object: {names:?}"
    );

    let properties = client
        .blob_client(&names[0])
        .get_properties(None)
        .await
        .unwrap();
    assert_eq!(
        properties.content_encoding().unwrap().as_deref(),
        Some("gzip"),
        "the stored blob must declare its gzip encoding"
    );
}

// --- Real Azure (Microsoft Entra ID) ---------------------------------------

/// `run` stores a harvested result set in a real Azure account via Entra ID.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Real-Azure network end-to-end test: self-skips without ENABLE_AZURE (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn run_stores_results_in_real_azure() {
    if !real_azure_enabled() {
        return;
    }
    with_real_azure_container(async |container| {
        scenario_run_stores(&real_azure_config(&container)).await;
    })
    .await;
}

/// A `run` + `analyze` round-trip through a real Azure account via Entra ID.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Real-Azure network end-to-end test: self-skips without ENABLE_AZURE (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn run_then_analyze_round_trips_through_real_azure() {
    if !real_azure_enabled() {
        return;
    }
    with_real_azure_container(async |container| {
        scenario_run_then_analyze(&real_azure_config(&container)).await;
    })
    .await;
}

/// A multi-commit feature/dirty round-trip through a real Azure account via Entra ID.
#[tokio::test]
#[cfg_attr(miri, ignore)]
#[cfg_attr(
    mutants,
    ignore = "Real-Azure network end-to-end test: self-skips without ENABLE_AZURE (as under mutation), and the azure IO it exercises is already mutants::skip"
)]
#[serial]
async fn analyze_feature_and_dirty_round_trip_through_real_azure() {
    if !real_azure_enabled() {
        return;
    }
    with_real_azure_container(async |container| {
        scenario_feature_and_dirty(&real_azure_config(&container)).await;
    })
    .await;
}
