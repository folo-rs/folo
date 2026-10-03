//! Immutable observation reuse and live Git interpretation at the native boundary.

use std::fs;

use crp_diag::{Discard, Verbose};
use crp_workspace::cache::{Cache, CacheOptions};
use crp_workspace::git::{CommitHeaders, GitObjectContext, HistoricalTree};
use tempfile::TempDir;

use crate::git_fixture::Repository;

fn repository() -> Repository {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\nmembers=[]\n");
    fixture.command(&["add", "Cargo.toml"]);
    fixture.command(&["commit", "--quiet", "-m", "root"]);
    fixture
}

#[test]
#[cfg_attr(
    miri,
    ignore = "acquires native Git replacement and cache observations"
)]
fn replacements_do_not_reuse_original_object_facts_and_removal_restores_eligibility() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
    )
    .unwrap();
    let git = fixture.repo();
    let root = git.rev_parse("HEAD").unwrap();
    let verbose = Verbose::new(false, &Discard);
    let original = GitObjectContext::capture(&git).unwrap();
    let tree = HistoricalTree::load(&git, &root, &original, &cache, verbose).unwrap();
    assert!(tree.entry("changed").is_none());
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &original, &cache, verbose)
            .unwrap()
    );

    fixture.write("changed", b"different tree");
    fixture.command(&["add", "changed"]);
    fixture.command(&["commit", "--quiet", "-m", "child"]);
    let child = git.rev_parse("HEAD").unwrap();
    fixture.command(&["replace", &root, &child]);
    let replaced = GitObjectContext::capture(&git).unwrap();
    assert_ne!(original, replaced);
    assert!(
        HistoricalTree::load(&git, &root, &replaced, &cache, verbose)
            .unwrap()
            .entry("changed")
            .is_some()
    );
    assert!(
        CommitHeaders::default()
            .has_parent(&git, &root, &replaced, &cache, verbose)
            .unwrap()
    );
    fixture.command(&["replace", "-d", &root]);
    let restored = GitObjectContext::capture(&git).unwrap();
    assert_eq!(original, restored);
    assert!(
        HistoricalTree::load(&git, &root, &restored, &cache, verbose)
            .unwrap()
            .entry("changed")
            .is_none()
    );
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &restored, &cache, verbose)
            .unwrap()
    );
}

#[test]
#[cfg_attr(miri, ignore = "changes local graft and shallow history observations")]
fn graft_context_changes_and_cached_headers_do_not_freeze_shallow_traversal() {
    let source = repository();
    let root = source.repo().rev_parse("HEAD").unwrap();
    source.command(&["commit", "--quiet", "--allow-empty", "-m", "child"]);
    let head = source.repo().rev_parse("HEAD").unwrap();
    let fixture = Repository::new();
    fixture.command(&[
        "fetch",
        "--quiet",
        "--depth=1",
        &source.path().to_string_lossy(),
        "HEAD",
    ]);
    fixture.command(&["checkout", "--quiet", "--detach", "FETCH_HEAD"]);
    let directory = TempDir::new().unwrap();
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
    )
    .unwrap();
    let git = fixture.repo();
    let context = GitObjectContext::capture(&git).unwrap();
    let verbose = Verbose::new(false, &Discard);
    let parent = CommitHeaders::default()
        .has_parent(&git, &head, &context, &cache, verbose)
        .unwrap();
    assert!(parent);
    assert!(git.parent_boundary_with_header(&head, parent).unwrap());
    assert_eq!(
        git.first_parent_commits(&head).unwrap(),
        std::slice::from_ref(&head)
    );
    fixture.command(&[
        "fetch",
        "--quiet",
        "--unshallow",
        &source.path().to_string_lossy(),
        "HEAD",
    ]);
    assert_eq!(GitObjectContext::capture(&git).unwrap(), context);
    assert!(
        CommitHeaders::default()
            .has_parent(&git, &head, &context, &cache, verbose)
            .unwrap()
    );
    assert_eq!(
        git.first_parent_commits(&head).unwrap(),
        [head.clone(), root]
    );
    assert!(!git.is_shallow().unwrap());

    let graft = fixture.path().join(".git").join("info").join("grafts");
    fs::create_dir_all(graft.parent().unwrap()).unwrap();
    fs::write(&graft, format!("{head}\n")).unwrap();
    assert_ne!(GitObjectContext::capture(&git).unwrap(), context);
    fs::remove_file(&graft).unwrap();
    assert_eq!(GitObjectContext::capture(&git).unwrap(), context);
}
