//! Native coverage of export destinations and the independently executable bundle.
//! Every Azure call in the standalone fixture is replaced in-process, never forwarded.

use std::fs as sync_fs;
use std::path::Path;
use std::process::Command as ProcessCommand;

use cargo_bench_history::{Command, Overrides, SetupAzureOptions, run_with_overrides};
use ohno::AppError;
use serde_json::{Value, from_slice};
use tempfile::{Builder, TempDir};
use tokio::fs;

fn workspace() -> TempDir {
    Builder::new()
        .prefix(".setup-azure-test-")
        .tempdir_in(env!("CARGO_MANIFEST_DIR"))
        .unwrap()
}

async fn export(root: &Path, options: SetupAzureOptions) -> Result<(), AppError> {
    run_with_overrides(
        &Command::SetupAzure(options),
        Overrides {
            workspace_dir: Some(root.to_path_buf()),
            ..Overrides::default()
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[cfg_attr(miri, ignore = "filesystem integration")]
async fn exports_without_a_repository_and_refuses_overwriting() {
    let workspace = workspace();
    let options = SetupAzureOptions {
        out_dir: Some("bundle".into()),
        ..SetupAzureOptions::default()
    };
    export(workspace.path(), options.clone()).await.unwrap();
    let bundle = workspace.path().join("bundle");
    let parameters: Value =
        from_slice(&fs::read(bundle.join("parameters.json")).await.unwrap()).unwrap();
    assert!(parameters.get("SubscriptionId").unwrap().is_null());
    fs::write(bundle.join("deploy.ps1"), "user-owned canary")
        .await
        .unwrap();
    export(workspace.path(), options).await.unwrap_err();
    assert_eq!(
        fs::read_to_string(bundle.join("deploy.ps1")).await.unwrap(),
        "user-owned canary"
    );
    assert!(!bundle.join("teardown.ps1").exists());
}

#[tokio::test]
#[cfg_attr(miri, ignore = "filesystem integration")]
async fn accepts_an_empty_directory_and_rejects_other_nonempty_destinations() {
    let workspace = workspace();
    fs::create_dir(workspace.path().join("empty"))
        .await
        .unwrap();
    export(
        workspace.path(),
        SetupAzureOptions {
            out_dir: Some("empty".into()),
            ..SetupAzureOptions::default()
        },
    )
    .await
    .unwrap();
    fs::create_dir(workspace.path().join("nonempty"))
        .await
        .unwrap();
    // A dot-prefixed entry covers a directory whose only content is hidden on Unix.
    fs::write(workspace.path().join("nonempty").join(".hidden"), "keep")
        .await
        .unwrap();
    export(
        workspace.path(),
        SetupAzureOptions {
            out_dir: Some("nonempty".into()),
            ..SetupAzureOptions::default()
        },
    )
    .await
    .unwrap_err();
    assert!(
        !workspace
            .path()
            .join("nonempty")
            .join("main.bicep")
            .exists()
    );
    fs::write(workspace.path().join("file"), "keep")
        .await
        .unwrap();
    export(
        workspace.path(),
        SetupAzureOptions {
            out_dir: Some("file".into()),
            ..SetupAzureOptions::default()
        },
    )
    .await
    .unwrap_err();
}

#[tokio::test]
#[cfg_attr(miri, ignore = "filesystem and PowerShell process integration")]
async fn extracted_driver_requires_missing_parameters_without_azure() {
    let workspace = workspace();
    export(
        workspace.path(),
        SetupAzureOptions {
            out_dir: Some("bundle".into()),
            ..SetupAzureOptions::default()
        },
    )
    .await
    .unwrap();
    let output = ProcessCommand::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(workspace.path().join("bundle").join("deploy.ps1"))
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("SubscriptionId"));
}

#[tokio::test]
#[cfg_attr(miri, ignore = "filesystem and PowerShell process integration")]
async fn extracted_bundle_deploys_with_literal_parameters_and_mocked_azure() {
    let workspace = workspace();
    // Whitespace, quotes and expression syntax exercise literal path and parameter handoff.
    export(
        workspace.path(),
        SetupAzureOptions {
            out_dir: Some("bundle space ' literal".into()),
            subscription_id: Some("00000000-0000-0000-0000-000000000001".into()),
            resource_group: Some("group'\"$() literal".into()),
            location: Some("westeurope".into()),
            storage_account: Some("examplehistory".into()),
            github_owner: Some("owner".into()),
            github_repository: Some("repository".into()),
            history_branch: Some("history/main".into()),
            ..SetupAzureOptions::default()
        },
    )
    .await
    .unwrap();
    let output = ProcessCommand::new("pwsh")
        .args(["-NoProfile", "-NonInteractive", "-File"])
        .arg(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("setup-azure.ps1"),
        )
        .arg("-BundleDirectory")
        .arg(workspace.path().join("bundle space ' literal"))
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("Deployment complete"));
    assert!(text.contains("client-canary"));
    assert!(text.contains("principal-canary"));
    assert!(text.contains("standalone-repeat-canary"));
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and binary process integration")]
fn binary_export_succeeds_without_any_tools_on_path() {
    let workspace = workspace();
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args(["setup-azure", "--out-dir", "bundle"])
        .env("PATH", "")
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(workspace.path().join("bundle").join("main.bicep").is_file());
}

#[test]
#[cfg_attr(miri, ignore = "filesystem and PowerShell process integration")]
fn binary_cleans_only_its_owned_bundle_after_driver_failure() {
    let workspace = workspace();
    let temporary_root = workspace.path().join("owned-temporary-parent");
    sync_fs::create_dir_all(&temporary_root).unwrap();
    sync_fs::write(temporary_root.join("keep"), "caller-owned").unwrap();
    // A reserved branch name reaches the driver, which rejects it before Azure CLI probes.
    // This exercises cleanup without depending on which Azure tools are installed.
    let output = ProcessCommand::new(env!("CARGO_BIN_EXE_cargo-bench-history"))
        .args([
            "setup-azure",
            "--subscription-id",
            "00000000-0000-0000-0000-000000000001",
            "--resource-group",
            "group",
            "--location",
            "westeurope",
            "--storage-account",
            "examplehistory",
            "--github-owner",
            "owner",
            "--github-repository",
            "repository",
            "--history-branch",
            "HEAD",
        ])
        .env("TMP", &temporary_root)
        .env("TEMP", &temporary_root)
        .env("TMPDIR", &temporary_root)
        .current_dir(workspace.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("HistoryBranch"));
    assert!(error.contains("Azure setup deployment"));
    let entries = sync_fs::read_dir(&temporary_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, ["keep"]);
}

::testing::set_allocator!();
