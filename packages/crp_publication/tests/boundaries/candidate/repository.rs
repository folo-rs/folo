use std::fs;

use crp_publication::publication::candidate::Repository;
use tempfile::TempDir;

use crate::candidate::repository_fixture::{command, fixture};
use crate::candidate::scheduling::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_evidence_changed_during_successful_or_failed_operations() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        repository.checked(|| Ok(())).unwrap();
        for succeeds in [true, false] {
            let result = repository.checked(|| {
                fs::write(repository.root().join("Cargo.toml"), "changed input").unwrap();
                if succeeds {
                    Ok(())
                } else {
                    Err(OperationError::new().into())
                }
            });
            let error = result.unwrap_err();
            assert!(error.find_source::<OperationError>().is_none());
            fs::write(repository.root().join("Cargo.toml"), "tracked input").unwrap();
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_head_changed_during_verification() {
    with_io_test(|| {
        let (directory, repository) = fixture();
        _ = repository
            .checked(|| {
                command(
                    &directory.path().join("repository"),
                    &["commit", "--allow-empty", "-m", "new head"],
                );
                Ok(())
            })
            .unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_dirty_inputs_before_invoking_the_operation() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        fs::write(repository.root().join("Cargo.toml"), "changed input").unwrap();
        let mut invoked = false;
        let result = repository.checked(|| {
            invoked = true;
            Ok(())
        });
        assert!(result.is_err());
        assert!(!invoked);
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_staged_and_untracked_inputs() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        fs::write(repository.root().join("Cargo.toml"), "staged input").unwrap();
        command(repository.root(), &["add", "Cargo.toml"]);
        _ = repository.ensure_clean_head().unwrap_err();

        fs::write(repository.root().join("Cargo.toml"), "tracked input").unwrap();
        command(repository.root(), &["add", "Cargo.toml"]);
        repository.ensure_clean_head().unwrap();
        fs::write(repository.root().join("untracked-input"), "untracked input").unwrap();
        _ = repository.ensure_clean_head().unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_index_flags_that_conceal_changes() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        for (set, unset) in [
            ("--assume-unchanged", "--no-assume-unchanged"),
            ("--skip-worktree", "--no-skip-worktree"),
        ] {
            repository.ensure_clean_head().unwrap();
            command(repository.root(), &["update-index", set, "Cargo.toml"]);
            fs::write(repository.root().join("Cargo.toml"), "concealed input").unwrap();
            assert!(command(repository.root(), &["status", "--porcelain"]).is_empty());
            _ = repository.ensure_clean_head().unwrap_err();
            fs::write(repository.root().join("Cargo.toml"), "tracked input").unwrap();
            command(repository.root(), &["update-index", unset, "Cargo.toml"]);
        }
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_noncommit_object_ids_for_either_history_endpoint() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let head = command(repository.root(), &["rev-parse", "HEAD"]);
        command(
            repository.root(),
            &["tag", "-a", "release", "-m", "annotated"],
        );
        let tag = command(repository.root(), &["rev-parse", "release"]);
        _ = repository.ensure_first_parent(tag.trim()).unwrap_err();

        let tagged =
            Repository::discover(&repository.root().join("Cargo.toml"), tag.trim()).unwrap();
        _ = tagged.ensure_first_parent(head.trim()).unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn preserves_operation_errors_when_evidence_is_unchanged() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let error = repository
            .checked(|| Err::<(), _>(OperationError::new().into()))
            .unwrap_err();
        assert!(error.find_source::<OperationError>().is_some());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Inspects native filesystem paths")]
fn rejects_missing_and_root_directory_manifest_paths() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        let absent = directory.path().join("absent.toml");
        _ = Repository::discover(&absent, "unused").unwrap_err();
        let root = directory.path().ancestors().last().unwrap();
        _ = Repository::discover(root, "unused").unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_a_manifest_outside_a_git_repository() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        let manifest = directory.path().join("Cargo.toml");
        fs::write(&manifest, "input").unwrap();
        _ = Repository::discover(&manifest, "unused").unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_inputs_outside_the_repository_or_ignored_by_git() {
    with_io_test(|| {
        let (directory, repository) = fixture();
        _ = repository
            .require_tracked(&directory.path().join("global-config"))
            .unwrap_err();
        fs::write(
            repository.root().join(".git/info/exclude"),
            "ignored-input\n",
        )
        .unwrap();
        let ignored = repository.root().join("ignored-input");
        fs::write(&ignored, "not tracked").unwrap();
        repository.ensure_clean_head().unwrap();
        _ = repository.require_tracked(&ignored).unwrap_err();
        assert_eq!(
            repository
                .require_tracked(&repository.root().join("Cargo.toml"))
                .unwrap(),
            repository.root().join("Cargo.toml")
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn propagates_an_unreadable_git_index() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        fs::write(repository.root().join(".git/index"), "invalid index").unwrap();
        _ = repository.ensure_clean_head().unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_an_unavailable_release_line() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        _ = repository.ensure_first_parent(&"0".repeat(40)).unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn propagates_an_unavailable_parent_in_release_history() {
    with_io_test(|| {
        let (_directory, repository) = fixture();
        let parent = command(repository.root(), &["rev-parse", "HEAD"])
            .trim()
            .to_owned();
        command(
            repository.root(),
            &["commit", "--allow-empty", "-m", "next snapshot"],
        );
        let head = command(repository.root(), &["rev-parse", "HEAD"])
            .trim()
            .to_owned();
        let repository =
            Repository::discover(&repository.root().join("Cargo.toml"), &head).unwrap();
        // The fixture disables automatic GC, so its parent is a loose object. Remove only that
        // owned object to exercise history traversal failure after commit resolution succeeds.
        let (prefix, suffix) = parent.split_at(2);
        fs::remove_file(
            repository
                .root()
                .join(".git/objects")
                .join(prefix)
                .join(suffix),
        )
        .unwrap();
        _ = repository.ensure_first_parent(&head).unwrap_err();
    });
}

/// Models a failed verification subprocess without running or replacing Cargo.
#[ohno::error]
struct OperationError;
