//! External acquisition for git.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

use crp_impl::command::run_capture;
use crp_impl::git::*;
use crp_impl::manifest::PathCase;
use tempfile::tempdir;

use crate::git_fixture as testing;

/// An empty pathspec list never reaches Git.
///
/// `git ls-files` with no pathspec lists the whole repository, which would report executable
/// paths from every package rather than only the one being classified.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn work_tree_modes_include_the_index() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("packages/foo")).unwrap();
    fs::write(root.join("packages/foo/run.sh"), "echo\n").unwrap();
    fs::write(root.join("packages/foo/lib.rs"), "x").unwrap();
    let repo = init_repo(root);
    // Set through the index, because a Windows checkout has no executable
    // permission to set and turns `core.fileMode` off.
    run_capture(
        "git",
        &["update-index", "--chmod=+x", "packages/foo/run.sh"],
        root,
    )
    .unwrap();
    #[cfg(unix)]
    {
        // Unix work-tree permissions are authoritative, so keep them aligned
        // with the index state this cross-platform test establishes.
        let path = root.join("packages/foo/run.sh");
        let mut permissions = fs::metadata(&path).unwrap().permissions();
        permissions.set_mode(permissions.mode() | 0o111);
        fs::set_permissions(path, permissions).unwrap();
    }

    let modes = repo
        .work_tree_modes(&["packages/foo"], PathCase::Sensitive)
        .unwrap();
    assert!(modes.is_executable("packages/foo/run.sh"));
    assert!(!modes.is_executable("packages/foo/lib.rs"));
    assert!(!modes.is_symlink("packages/foo/run.sh"));
    assert_eq!(
        repo.work_tree_modes(&[], PathCase::Sensitive).unwrap(),
        WorkTreeModes::default()
    );
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn an_option_like_revision_resolves_only_as_a_ref() {
    let temp = tempdir().unwrap();
    let repo = init_repo(temp.path());
    let expected = repo.rev_parse("HEAD").unwrap();
    // Git accepts this full ref even though the shorthand is also an option.
    run_capture(
        "git",
        &["update-ref", "refs/heads/--all", "HEAD"],
        temp.path(),
    )
    .unwrap();

    assert_eq!(repo.rev_parse("--all").unwrap(), expected);
}

/// Discover reports a prefix for an uncanonical directory.
///
/// Cargo and Git need not spell the same directory identically: Windows hands out 8.3 short
/// names for some paths and both tools accept uncanonical spellings, so the prefix must come
/// from Git rather than from subtracting one reported path from the other.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn discover_reports_a_prefix_for_an_uncanonical_directory() {
    let temp = tempdir().unwrap();
    run_capture("git", &["init", "-q"], temp.path()).unwrap();
    let nested = temp.path().join("inner");
    fs::create_dir_all(&nested).unwrap();

    let repo = GitRepo::discover(&nested.join("..").join("inner")).unwrap();

    assert_eq!(repo.prefix(), "inner");
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn discover_reports_an_empty_prefix_at_the_repository_root() {
    let temp = tempdir().unwrap();
    run_capture("git", &["init", "-q"], temp.path()).unwrap();

    let repo = GitRepo::discover(temp.path()).unwrap();

    assert_eq!(repo.prefix(), "");
    assert!(
        repo.root().ends_with(
            temp.path()
                .file_name()
                .expect("a temporary directory always has a final component")
        )
    );
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn discover_starts_from_the_directory_holding_a_file() {
    let temp = tempdir().unwrap();
    run_capture("git", &["init", "-q"], temp.path()).unwrap();
    let nested = temp.path().join("inner");
    fs::create_dir_all(&nested).unwrap();
    let manifest = nested.join(MANIFEST_FILE_NAME);
    fs::write(&manifest, "").unwrap();

    let repo = GitRepo::discover(&manifest).unwrap();

    assert_eq!(repo.prefix(), "inner");
}

#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn discover_fails_outside_a_repository() {
    let temp = tempdir().unwrap();

    GitRepo::discover(temp.path()).unwrap_err();
}

/// Listings fail when the root is not a repository.
///
/// Every listing runs `git` in the repository root, so a root that is not a repository must
/// surface the failure rather than an empty listing.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn listings_fail_when_the_root_is_not_a_repository() {
    let temp = tempdir().unwrap();
    let repo = GitRepo {
        root: temp.path().to_path_buf(),
        prefix: String::new(),
    };

    repo.first_parent_commits("HEAD").unwrap_err();
    repo.ls_files("").unwrap_err();
    repo.ls_untracked("", PathCase::Sensitive).unwrap_err();
    repo.ls_tree("HEAD", &[""]).unwrap_err();
    repo.ls_tree_paths("HEAD").unwrap_err();
    repo.hash_objects(&["Cargo.toml"]).unwrap_err();
    repo.rev_parse("HEAD").unwrap_err();
}

/// Show file distinguishes an absent path from a failure.
///
/// A path absent at a commit is an ordinary answer, while any other `git show` failure is a
/// real error the caller must see.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn show_file_distinguishes_an_absent_path_from_a_failure() {
    let temp = tempdir().unwrap();
    let repo = init_repo_with_two_commits(temp.path());

    assert_eq!(
        repo.show_file("HEAD", "first.txt").unwrap().as_deref(),
        Some("one\n")
    );
    assert_eq!(repo.show_file("HEAD", "absent.txt").unwrap(), None);
    repo.show_file("no-such-revision", "first.txt").unwrap_err();
}

fn init_repo_with_two_commits(root: &Path) -> GitRepo {
    let repo = init_repo(root);
    fs::write(root.join("second.txt"), "two\n").unwrap();
    run_capture("git", &["add", "-A"], root).unwrap();
    run_capture("git", &["commit", "-q", "-m", "second"], root).unwrap();
    repo
}

/// Initialises a hermetic repository and commits whatever `root` holds.
fn init_repo(root: &Path) -> GitRepo {
    run_capture("git", &["init", "-q"], root).unwrap();
    run_capture("git", &["config", "user.name", "test"], root).unwrap();
    run_capture("git", &["config", "user.email", "test@example.com"], root).unwrap();
    run_capture("git", &["config", "commit.gpgsign", "false"], root).unwrap();
    fs::write(root.join("first.txt"), "one\n").unwrap();
    run_capture("git", &["add", "-A"], root).unwrap();
    run_capture("git", &["commit", "-q", "-m", "first"], root).unwrap();
    GitRepo::discover(root).unwrap()
}

/// A directory named like a pattern lists only its own files.
///
/// The literal pathspec must survive the round trip through Git itself: the escaping is only
/// correct if Git reads it back as one plain path.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn a_directory_named_like_a_pattern_lists_only_its_own_files() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    // `de[m]o` is a legal directory name on every supported platform and is
    // also a glob that matches its sibling `demo`.
    for dir in ["packages/de[m]o", "packages/demo"] {
        fs::create_dir_all(root.join(dir)).unwrap();
        fs::write(root.join(dir).join("lib.rs"), "x").unwrap();
    }
    let repo = init_repo(root);

    assert_eq!(
        repo.ls_files("packages/de[m]o").unwrap(),
        vec!["packages/de[m]o/lib.rs".to_string()]
    );
    let entries = repo.ls_tree("HEAD", &["packages/de[m]o"]).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        vec!["packages/de[m]o/lib.rs".to_string()]
    );
}

/// A work tree file hashes to the id its tree entry records.
///
/// Git converts content on its way into the object database, so the id a work-tree file hashes
/// to is the representation both ends of a comparison have to be expressed in. It must agree
/// with the id the tree records for an unmodified file.
#[cfg_attr(miri, ignore)] // Spawns git, which Miri cannot emulate.
#[test]
fn a_work_tree_file_hashes_to_the_id_its_tree_entry_records() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    fs::create_dir_all(root.join("packages/demo")).unwrap();
    fs::write(root.join("packages/demo/lib.rs"), "x").unwrap();
    fs::write(root.join("packages/demo/other.rs"), "y").unwrap();
    let repo = init_repo(root);

    let entries = repo.ls_tree("HEAD", &["packages/demo"]).unwrap();
    let recorded: Vec<String> = entries.iter().map(|entry| entry.id.clone()).collect();
    let hashed = repo
        .hash_objects(&["packages/demo/lib.rs", "packages/demo/other.rs"])
        .unwrap();

    assert_eq!(hashed, recorded);
    for (id, bytes) in hashed.iter().zip([b"x", b"y"]) {
        assert_eq!(repo.show_blob_bytes(id).unwrap(), bytes);
    }
    repo.show_blob_bytes("missing-object").unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "queries a real Git index")]
fn tracked_resource_queries_keep_recorded_paths_and_exclude_untracked_files() {
    let fixture = testing::Repository::new();
    fixture.write("shared/Guide.md", b"tracked");
    fixture.write("shared/untracked.md", b"not staged");
    fixture.command(&["add", "shared/Guide.md"]);
    let repo = fixture.repo();

    assert_eq!(
        repo.tracked_paths(
            &[
                "shared/Guide.md",
                "shared/untracked.md",
                "shared/missing.md"
            ],
            PathCase::Sensitive,
        )
        .unwrap(),
        ["shared/Guide.md"]
    );
    assert!(
        repo.tracked_paths(&[], PathCase::Sensitive)
            .unwrap()
            .is_empty()
    );
    assert!(
        repo.tracked_paths(&["shared/guide.md"], PathCase::Sensitive)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.tracked_paths(&["shared/guide.md"], PathCase::Insensitive)
            .unwrap(),
        ["shared/Guide.md"]
    );
}
