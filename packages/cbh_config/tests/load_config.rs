//! Filesystem coverage for configuration acquisition; parsing and read policy stay unit-tested.

#![cfg(not(miri))]

use std::{fs, io};

use cbh_config::{CloudStorageConfig, Config, load_config};
use ohno::ErrorExt;
use tempfile::tempdir;

::testing::set_allocator!();

#[tokio::test]
async fn reads_and_parses_config() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bench_history.toml");
    fs::write(
        &path,
        "[project]\nid = \"folo\"\n\n[storage.azure]\naccount = \"a\"\ncontainer = \"c\"\n",
    )
    .unwrap();

    let config = load_config(&path, true).await.unwrap();

    assert!(matches!(config.storage, Some(CloudStorageConfig::Azure(_))));
    assert_eq!(config.project.id.as_deref(), Some("folo"));
}

#[tokio::test]
async fn missing_explicit_file_is_read_error() {
    let dir = tempdir().unwrap();
    let error = load_config(&dir.path().join("absent.toml"), true)
        .await
        .unwrap_err();

    assert_eq!(
        error.find_source::<io::Error>().unwrap().kind(),
        io::ErrorKind::NotFound
    );
}

#[tokio::test]
async fn missing_default_file_yields_empty_config() {
    let dir = tempdir().unwrap();
    let config = load_config(&dir.path().join("absent.toml"), false)
        .await
        .unwrap();

    assert_eq!(config, Config::default());
}
