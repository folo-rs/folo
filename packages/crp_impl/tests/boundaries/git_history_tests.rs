//! External acquisition for git.

use std::slice;

use crp_impl::UnresolvedBaseError;
use crp_impl::manifest::PathCase;

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(miri, ignore = "creates Git history and spawns Git")]
fn a_commit_with_a_reachable_parent_is_not_a_root() {
    let fixture = Repository::new();
    let repo = fixture.repo();
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "root"]);
    let root = repo.rev_parse("HEAD").unwrap();
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "child"]);
    let head = repo.rev_parse("HEAD").unwrap();

    assert!(repo.has_parent_or_is_shallow_boundary(&head).unwrap());
    assert!(!repo.has_parent_or_is_shallow_boundary(&root).unwrap());
    assert!(!repo.is_shallow().unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "creates Git history and spawns Git")]
fn a_root_commit_whose_message_mentions_a_parent_is_still_a_root() {
    let fixture = Repository::new();
    fixture.command(&[
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "subject",
        "-m",
        // This is message text, not a header or an object that Git must resolve.
        "parent mentioned in the commit message",
    ]);
    let repo = fixture.repo();
    let root = repo.rev_parse("HEAD").unwrap();

    // Check the acquired header fact itself: the full-repository fallback could
    // otherwise hide a root incorrectly reported as having a parent header.
    assert!(!repo.commit_has_parent_header(&root).unwrap());
    assert!(!repo.has_parent_or_is_shallow_boundary(&root).unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "fetches local Git history into a shallow repository")]
fn shallow_boundaries_and_true_roots_remain_distinct() {
    let source = Repository::new();
    source.command(&["commit", "--quiet", "--allow-empty", "-m", "root"]);
    let root = source.repo().rev_parse("HEAD").unwrap();
    source.command(&["commit", "--quiet", "--allow-empty", "-m", "child"]);
    let head = source.repo().rev_parse("HEAD").unwrap();

    let fixture = Repository::new();
    fixture.command(&[
        "fetch",
        "--quiet",
        "--no-tags",
        "--depth=1",
        &source.path().to_string_lossy(),
        "HEAD",
    ]);
    let repo = fixture.repo();
    assert!(repo.is_shallow().unwrap());
    assert_eq!(
        repo.first_parent_commits(&head).unwrap(),
        slice::from_ref(&head)
    );
    assert!(
        repo.rev_parse(&format!("{head}^"))
            .unwrap_err()
            .find_source::<UnresolvedBaseError>()
            .is_some()
    );
    assert!(repo.has_parent_or_is_shallow_boundary(&head).unwrap());
    assert_eq!(
        repo.first_parent_manifest_commits(&head, PathCase::Sensitive)
            .unwrap(),
        slice::from_ref(&head)
    );

    // A repository can contain both a truncated branch and a complete root.
    // Its shallow flag must not turn every parentless revision into a boundary.
    fixture.command(&[
        "fetch",
        "--quiet",
        "--no-tags",
        "--depth=1",
        &source.path().to_string_lossy(),
        &root,
    ]);
    assert!(repo.is_shallow().unwrap());
    assert!(!repo.has_parent_or_is_shallow_boundary(&root).unwrap());
    assert!(repo.has_parent_or_is_shallow_boundary(&head).unwrap());
}

#[test]
#[cfg_attr(miri, ignore = "creates and merges Git history")]
fn manifest_history_retains_endpoints_and_first_parent_manifest_changes() {
    let fixture = Repository::new();
    fixture.command(&["checkout", "--quiet", "-b", "main"]);
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "root"]);
    let repo = fixture.repo();
    let root = repo.rev_parse("HEAD").unwrap();

    fixture.write("Cargo.toml", b"[workspace]\n");
    fixture.command(&["add", "Cargo.toml"]);
    fixture.command(&["commit", "--quiet", "-m", "root manifest"]);
    let manifest = repo.rev_parse("HEAD").unwrap();
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "unrelated"]);
    let unrelated = repo.rev_parse("HEAD").unwrap();

    fixture.command(&["checkout", "--quiet", "-b", "topic"]);
    fixture.write("nested/Cargo.toml", b"[package]\n");
    fixture.command(&["add", "nested/Cargo.toml"]);
    fixture.command(&["commit", "--quiet", "-m", "nested manifest"]);
    fixture.command(&["checkout", "--quiet", "main"]);
    fixture.command(&[
        "merge",
        "--quiet",
        "--no-ff",
        "-m",
        "merge manifest",
        "topic",
    ]);
    let merge = repo.rev_parse("HEAD").unwrap();
    fixture.command(&["commit", "--quiet", "--allow-empty", "-m", "tip"]);
    let tip = repo.rev_parse("HEAD").unwrap();

    assert_eq!(
        repo.first_parent_commits(&tip).unwrap(),
        [
            tip.clone(),
            merge.clone(),
            unrelated,
            manifest.clone(),
            root.clone()
        ]
    );
    for case in [PathCase::Sensitive, PathCase::Insensitive] {
        assert_eq!(
            repo.first_parent_manifest_commits(&tip, case).unwrap(),
            [tip.clone(), merge.clone(), manifest.clone(), root.clone()]
        );
    }
}
