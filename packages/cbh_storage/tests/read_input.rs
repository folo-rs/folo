//! Real-filesystem coverage for read-input selection and cache-directory isolation.
//! Azure construction uses a transport and credential that reject all network work.
#![cfg(not(miri))]

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::Path;
use std::sync::Arc;

use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use azure_core::http::{AsyncRawResponse, HttpClient, Request};
use cbh_config::Config;
use cbh_diag::RecordingReporter;
use cbh_storage::{
    LocalStorage, ReadStorage, Storage, StorageError, StorageFacade, azure_backend_from_parts,
    build_storage, resolve_read_storage,
};
use same_file::is_same_file;
use tempfile::tempdir;

/// An offline Azure dependency that makes unexpected cloud work fail immediately.
#[derive(Debug)]
struct OfflineAzure;

#[async_trait::async_trait]
impl TokenCredential for OfflineAzure {
    async fn get_token(
        &self,
        _: &[&str],
        _: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        panic!("directory validation must not request an Azure token")
    }
}

#[async_trait::async_trait]
impl HttpClient for OfflineAzure {
    async fn execute_request(&self, _: &Request) -> azure_core::Result<AsyncRawResponse> {
        panic!("directory validation must not send an Azure request")
    }
}

async fn read_storage(
    base: &Path,
    input: &Path,
    mirror: Option<&Path>,
) -> Result<ReadStorage<StorageFacade, LocalStorage>, StorageError> {
    let azure = azure_backend_from_parts(
        "account",
        "history",
        Some("https://storage.example.test".to_owned()),
        Arc::new(OfflineAzure),
        Arc::new(OfflineAzure),
    )
    .unwrap()
    .into_facade();
    resolve_read_storage(
        Some(azure),
        None,
        &Config::default(),
        base,
        mirror,
        Some(input),
        &RecordingReporter::new(),
    )
    .await
}

#[tokio::test]
async fn overlapping_input_and_cache_directories_are_rejected_without_changes() {
    let root = tempdir().unwrap();
    let input = root.path().join("input");
    fs::create_dir_all(&input).unwrap();
    let sentinel = input.join("sentinel");
    fs::write(&sentinel, b"preserve input").unwrap();
    for mirror in [input.as_path(), root.path(), &input.join("missing-cache")] {
        read_storage(root.path(), &input, Some(mirror))
            .await
            .unwrap_err();
        assert_eq!(fs::read(&sentinel).unwrap(), b"preserve input");
    }
    assert!(!input.join("missing-cache").exists());
}

#[tokio::test]
async fn a_disjoint_uncreated_cache_does_not_absorb_local_results() {
    let root = tempdir().unwrap();
    let input = root.path().join("input");
    let mirror = root.path().join("cache");
    let local = build_storage(Some(&input), &Config::default(), root.path(), None).unwrap();
    local.put("project/tip", b"current").await.unwrap();
    let stored = input.join("project").join("tip");
    let before = fs::read(&stored).unwrap();
    let storage = read_storage(root.path(), &input, Some(&mirror))
        .await
        .unwrap();

    assert_eq!(storage.get("project/tip").await.unwrap(), b"current");
    storage.put("project/new", b"new").await.unwrap_err();
    storage
        .put_overwrite("project/tip", b"replacement")
        .await
        .unwrap_err();
    storage.delete("project/tip").await.unwrap_err();
    assert_eq!(fs::read(&stored).unwrap(), before);
    assert!(!input.join("project").join("new").exists());
    assert!(!mirror.exists());
}

#[tokio::test]
async fn case_aliases_follow_actual_filesystem_identity() {
    let root = tempdir().unwrap();
    let mirror = root.path().join("cache");
    let input = root.path().join("CACHE");
    fs::create_dir_all(&mirror).unwrap();
    fs::create_dir_all(&input).unwrap();
    let aliased = is_same_file(&mirror, &input).unwrap();
    let result = read_storage(root.path(), &input, Some(&mirror)).await;
    assert_eq!(result.is_err(), aliased);
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_link_aliases_are_rejected() {
    let root = tempdir().unwrap();
    let input = root.path().join("input");
    let alias = root.path().join("alias");
    fs::create_dir_all(&input).unwrap();
    symlink(&input, &alias).unwrap();
    read_storage(root.path(), &input, Some(&alias.join("nested")))
        .await
        .unwrap_err();
}

#[tokio::test]
async fn missing_and_non_directory_inputs_are_errors() {
    let root = tempdir().unwrap();
    let missing = root.path().join("missing");
    let error = read_storage(root.path(), &missing, None).await.unwrap_err();
    assert!(!error.is_not_found());
    assert_eq!(error.already_existing_key(), None);
    assert!(!missing.exists());
    let file = root.path().join("file");
    fs::write(&file, b"not a directory").unwrap();
    read_storage(root.path(), &file, None).await.unwrap_err();
    assert_eq!(fs::read(&file).unwrap(), b"not a directory");
}
