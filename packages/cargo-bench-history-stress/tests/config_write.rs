//! Exercises the configuration writer's real Tokio filesystem adapter.
//!
//! Backend serialization is unit-tested in memory; these fixtures exercise parent
//! creation, complete replacement and filesystem failures without provisioning storage.

use std::fs;

use cargo_bench_history_stress::write_config;
use tempfile::TempDir;

#[tokio::test]
#[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
async fn creates_parents_and_replaces_backend_configuration() {
    let dir = TempDir::new().unwrap();
    let workspace = dir.path().join("workspace");
    let path = workspace.join(".cargo").join("bench_history.toml");
    let local = "[project]\nid = \"stress\"\n";
    let azure = "[project]\nid = \"stress\"\n\n[storage.azure]\naccount = \"account\"\n\
                 container = \"container\"\nendpoint = \"https://account.blob.core.windows.net\"\n";

    // Replacing the longer cloud configuration also proves the file is truncated.
    for contents in [local, azure, local, ""] {
        assert_eq!(write_config(&workspace, contents).await.unwrap(), path);
        assert_eq!(fs::read_to_string(&path).unwrap(), contents);
    }
}

#[tokio::test]
#[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
async fn reports_parent_creation_failure() {
    let dir = TempDir::new().unwrap();
    let parent = dir.path().join(".cargo");
    fs::write(&parent, "not a directory").unwrap();

    _ = write_config(dir.path(), "configuration").await.unwrap_err();
    assert_eq!(fs::read_to_string(parent).unwrap(), "not a directory");
}

#[tokio::test]
#[cfg_attr(miri, ignore = "uses the real filesystem and Tokio runtime")]
async fn reports_file_write_failure() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join(".cargo").join("bench_history.toml");
    fs::create_dir_all(&path).unwrap();

    _ = write_config(dir.path(), "configuration").await.unwrap_err();
    assert!(path.is_dir());
}
