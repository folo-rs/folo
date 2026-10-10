//! Native filesystem coverage of local write-once publication and its failure cleanup.

use std::fs::{File, create_dir_all, read_dir};
use std::io;

use cbh_storage::{LocalStorage, Storage};
use ohno::ErrorExt as _;
use tempfile::tempdir;

::testing::set_allocator!();

#[tokio::test]
#[cfg_attr(miri, ignore = "exercises native filesystem path handling")]
async fn publication_supports_long_object_paths() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path());
    // Short components together exceed the legacy Windows path-length limit.
    let key = format!("{}object.json", "nested/".repeat(40));
    storage.put(&key, b"payload").await.unwrap();
    assert_eq!(storage.get(&key).await.unwrap(), b"payload");
    let error = storage.put(&key, b"collision").await.unwrap_err();
    assert_eq!(error.already_existing_key(), Some(key.as_str()));
    storage.put_overwrite(&key, b"replacement").await.unwrap();
    assert_eq!(storage.get(&key).await.unwrap(), b"replacement");
}

#[tokio::test]
#[cfg_attr(miri, ignore = "compares permissions created by the real filesystem")]
async fn publication_uses_standard_file_creation_permissions() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path());
    let control = File::create(dir.path().join("control")).unwrap();
    storage.put("object.json", b"payload").await.unwrap();

    assert_eq!(
        dir.path()
            .join("object.json")
            .metadata()
            .unwrap()
            .permissions(),
        control.metadata().unwrap().permissions()
    );
}

#[tokio::test]
#[cfg_attr(miri, ignore = "exercises real filesystem publication")]
async fn competing_writes_keep_exactly_one_complete_object() {
    let dir = tempdir().unwrap();
    let first = LocalStorage::new(dir.path());
    let second = LocalStorage::new(dir.path());
    let key = "v1/folo/objects/blessings/anchor/bless-123.json";

    let (first_result, second_result) = tokio::join!(
        first.put(key, b"first acceptance"),
        second.put(key, b"second acceptance"),
    );
    let (winner, collision) = match (first_result, second_result) {
        (Ok(()), Err(error)) => (b"first acceptance".as_slice(), error),
        (Err(error), Ok(())) => (b"second acceptance".as_slice(), error),
        result => panic!("expected one successful publication and one collision: {result:?}"),
    };

    assert_eq!(collision.already_existing_key(), Some(key));
    assert_eq!(first.get(key).await.unwrap(), winner);
    assert_eq!(first.list("v1/folo/").await.unwrap(), [key]);
    // Inspect disk as well: list deliberately hides abandoned staging files.
    let parent = dir
        .path()
        .join("v1")
        .join("folo")
        .join("objects")
        .join("blessings")
        .join("anchor");
    assert_eq!(read_dir(parent).unwrap().count(), 1);
}

#[tokio::test]
#[cfg_attr(miri, ignore = "exercises real filesystem publication")]
async fn collision_cleanup_preserves_the_winner_and_allows_explicit_overwrite() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path());
    storage.put("object.json", b"first").await.unwrap();
    let error = storage.put("object.json", b"second").await.unwrap_err();
    assert_eq!(error.already_existing_key(), Some("object.json"));
    assert_eq!(storage.get("object.json").await.unwrap(), b"first");
    assert_eq!(read_dir(dir.path()).unwrap().count(), 1);

    storage
        .put_overwrite("object.json", b"replacement")
        .await
        .unwrap();
    assert_eq!(storage.get("object.json").await.unwrap(), b"replacement");
    assert_eq!(read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
#[cfg_attr(miri, ignore = "exercises real filesystem publication")]
async fn failed_publication_is_not_a_collision_and_removes_staging() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path());
    create_dir_all(dir.path().join("directory")).unwrap();

    // A complete staged file cannot replace a directory.
    let error = storage
        .put_overwrite("directory", b"payload")
        .await
        .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
    assert_eq!(error.already_existing_key(), None);
    assert!(dir.path().join("directory").is_dir());
    assert_eq!(read_dir(dir.path()).unwrap().count(), 1);
}

#[tokio::test]
#[cfg_attr(
    miri,
    ignore = "checks that malformed input creates no filesystem objects"
)]
async fn nul_in_a_key_never_publishes_a_truncated_filename() {
    let dir = tempdir().unwrap();
    let storage = LocalStorage::new(dir.path());
    let error = storage.put("bad\0name", b"payload").await.unwrap_err();
    assert_eq!(error.already_existing_key(), None);
    assert!(!error.is_not_found());
    assert_eq!(read_dir(dir.path()).unwrap().count(), 0);
}
