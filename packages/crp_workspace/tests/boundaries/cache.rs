//! Immutable observation reuse and live Git interpretation at the native boundary.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crp_diag::{Discard, Verbose};
use crp_workspace::cache::{Cache, CacheOptions};
use crp_workspace::git::{CommitHeaders, GitObjectContext, HistoricalTree};
use crp_workspace::manifest::PathCase;
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
#[cfg_attr(miri, ignore = "resolves native Git administration and path aliases")]
fn administration_and_aliased_reserved_inputs_are_not_disposable_storage() {
    let fixture = repository();
    let linked = TempDir::new().unwrap();
    fixture.command(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        linked.path().to_str().unwrap(),
        "HEAD",
    ]);
    for directory in [
        linked.path().join(".git"),
        fixture.path().join(".git"),
        fixture.path().join(".git/unused-cache"),
    ] {
        let error = Cache::resolve(
            &linked.path().join("Cargo.toml"),
            &CacheOptions::Directory(directory),
            Verbose::new(false, &Discard),
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlaps"));
    }
    if PathCase::probe(fixture.path()) == PathCase::Insensitive {
        let alias = fixture.path().to_string_lossy().to_uppercase();
        let error = Cache::resolve(
            &Path::new(&alias).join("Cargo.toml"),
            &CacheOptions::Directory(fixture.path().join(".cargo/config.toml")),
            Verbose::new(false, &Discard),
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlaps"));
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "probes native read-only directory case rules")]
fn evidence_case_admission_uses_existing_entries_in_read_only_directories() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    fs::write(directory.path().join("probe.txt"), "").unwrap();
    let case = PathCase::probe(directory.path());
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    let original = fs::metadata(directory.path()).unwrap().permissions();
    // Remove all write bits while retaining the fixture's existing read/search permissions.
    fs::set_permissions(
        directory.path(),
        fs::Permissions::from_mode(original.mode() & !0o222),
    )
    .unwrap();
    let result = cache.protect(&directory.path().join("CACHE/report"));
    fs::set_permissions(directory.path(), original).unwrap();
    assert_eq!(result.is_ok(), case == PathCase::Sensitive);
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
        Verbose::new(false, &Discard),
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
        Verbose::new(false, &Discard),
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
