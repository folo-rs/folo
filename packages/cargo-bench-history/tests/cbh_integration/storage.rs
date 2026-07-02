//! Storage-selection behavior: how a command resolves where to read and write
//! results from the `--local` flag, the `CARGO_BENCH_HISTORY_STORAGE`
//! environment variable, and the configured cloud backend, in that precedence.
//!
//! The bare-`--local`-from-environment and missing-environment paths read the
//! real process environment, so they are exercised against the actual binary in
//! [`cli`](super::cli) (each child gets its own environment) rather than in
//! process here.

use crate::harness::*;

/// A configuration carrying a project id plus an Azure cloud backend, used to
/// prove `--local` overrides a configured cloud backend. The endpoint is never
/// contacted: `--local` wins, so the Azure block is parsed but never built.
fn cloud_config() -> String {
    "[project]\n\
     id = \"testproj\"\n\n\
     [storage.azure]\n\
     account = \"devstoreaccount1\"\n\
     container = \"bench-history\"\n"
        .to_owned()
}

/// An explicit relative `--local` path is taken relative to the workspace, so
/// the store lands inside it regardless of the process current directory.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_local_flag_with_a_relative_path_rebases_against_the_workspace() {
    // `--local=store` is supplied explicitly (no harness injection), so this
    // exercises the real relative-path rebasing the flag performs.
    let workspace = Workspace::new(&storage_only_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run", "--local=store"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    // `stored_objects` reads `<workspace>/store`, so a result there proves the
    // relative path resolved against the workspace, not the process cwd.
    let (key, _) = workspace.single_object();
    assert!(key.starts_with("v1/testproj/objects/"), "{key}");
}

/// `--local` overrides a configured cloud backend: with both present the local
/// path is used and the cloud backend is never built (its endpoint is never
/// contacted), so the run completes and stores locally.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_local_flag_overrides_a_configured_cloud_backend() {
    let workspace = Workspace::new(&cloud_config()).with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run", "--local=store"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Stored 1"), "{message}");

    // The object landed in the local store, proving the cloud backend was not
    // used despite being configured.
    let (key, _) = workspace.single_object();
    assert!(key.starts_with("v1/testproj/objects/"), "{key}");
}

/// With neither `--local` nor a configured cloud backend, a storage-backed
/// command fails fast with a storage configuration error before launching any
/// benchmark.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_without_a_storage_selection_or_cloud_config_errors() {
    let workspace = Workspace::new(&storage_only_config())
        .without_local_storage()
        .with_bench(&["--summary", "grp=single"]);

    let error = workspace.drive(&["run"]).await.unwrap_err();
    let RunError::Storage(storage) = error else {
        panic!("expected a storage error, got {error:?}");
    };
    let rendered = storage.to_string();
    assert!(rendered.contains("no storage configured"), "{rendered}");

    assert!(
        workspace.stored_objects().is_empty(),
        "a failed selection stores nothing"
    );
}

/// Drives `args` against a workspace that has a project id but no storage
/// selection and no configured cloud backend, asserting the command fails fast
/// with the `build_storage` "no storage configured" error. Every storage-backed
/// command resolves its backend the same way, so this covers each command's
/// `build_storage(...)?` call site propagating the configuration error rather than
/// proceeding to touch git or the network.
async fn assert_command_errors_without_storage(args: &[&str]) {
    let workspace = Workspace::new(&storage_only_config()).without_local_storage();

    let error = workspace.drive(args).await.unwrap_err();
    let RunError::Storage(storage) = error else {
        panic!("expected a storage error from {args:?}, got {error:?}");
    };
    let rendered = storage.to_string();
    assert!(
        rendered.contains("no storage configured"),
        "{args:?}: {rendered}"
    );
}

/// `analyze` fails fast with the storage configuration error when no backend is
/// selected or configured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn analyze_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["analyze"]).await;
}

/// `list` fails fast with the storage configuration error when no backend is
/// selected or configured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn list_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["list", "runs"]).await;
}

/// `prune` fails fast with the storage configuration error when no backend is
/// selected or configured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn prune_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["prune", "--clean"]).await;
}

/// `bless` fails fast with the storage configuration error when no backend is
/// selected or configured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn bless_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["bless", "--all"]).await;
}

/// `unbless` fails fast with the storage configuration error when no backend is
/// selected or configured.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn unbless_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["unbless"]).await;
}

/// `backfill` fails fast with the storage configuration error when no backend is
/// selected or configured, before any commit in its range is resolved.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn backfill_without_a_storage_selection_or_cloud_config_errors() {
    assert_command_errors_without_storage(&["backfill", "HEAD~1", "HEAD"]).await;
}

/// `--no-store` skips storage selection entirely, so the run completes and
/// stores nothing even with no `--local` and no configured cloud backend.
#[tokio::test]
#[cfg_attr(miri, ignore)]
async fn run_no_store_needs_no_storage_selection() {
    let workspace = Workspace::new(&storage_only_config())
        .without_local_storage()
        .with_bench(&["--summary", "grp=single"]);

    let outcome = workspace.drive(&["run", "--no-store"]).await.unwrap();
    let RunOutcome::Completed { message } = outcome else {
        panic!("expected completion, got {outcome:?}");
    };
    assert!(message.contains("Harvested 1"), "{message}");
    assert!(message.contains("nothing stored"), "{message}");

    assert!(
        workspace.stored_objects().is_empty(),
        "nothing should be stored under --no-store"
    );
}
