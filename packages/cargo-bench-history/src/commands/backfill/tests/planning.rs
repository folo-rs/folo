use super::*;

#[test]
fn project_prefix_keeps_root_projects_at_the_worktree_root() {
    for output in ["", "\n", "\r\n"] {
        let relative = parse_project_prefix(output).unwrap();
        assert_eq!(relative, Path::new(""));
        assert_eq!(worktree().join(relative), worktree());
    }
}

#[test]
fn project_prefix_preserves_nested_names_and_spaces() {
    for output in [
        " .github/Fixtures/Nested Project/",
        " .github/Fixtures/Nested Project/\n",
        " .github/Fixtures/Nested Project/\r\n",
    ] {
        let relative = parse_project_prefix(output).unwrap();
        let expected = Path::new(" .github")
            .join("Fixtures")
            .join("Nested Project");
        assert_eq!(relative, expected);
        assert_eq!(worktree().join(relative), worktree().join(expected));
    }
}

#[test]
fn project_prefix_rejects_paths_outside_the_worktree() {
    for output in [
        "../outside\n",
        "nested/../../outside\n",
        "/outside\n",
        "./nested\n",
    ] {
        let error = parse_project_prefix(output).unwrap_err();
        assert!(error.find_source::<BackfillError>().is_some());
    }
}

#[test]
#[cfg(windows)]
fn project_prefix_rejects_windows_drive_and_unc_paths() {
    for output in [
        r"C:\outside",
        "C:outside",
        r"\\server\share\outside",
        r"\outside",
    ] {
        let error = parse_project_prefix(output).unwrap_err();
        assert!(error.find_source::<BackfillError>().is_some());
    }
}

#[test]
fn plan_enumerates_inclusive_first_parent_range_newest_first() {
    let git = FakeBackfillGit::new(fixture());
    let commits = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap();
    assert!(
        commits.iter().eq(["f2", "f1", "c1"].iter()),
        "inclusive of both endpoints, newest first: {commits:?}"
    );
}

#[test]
fn plan_includes_a_single_commit_range() {
    let git = FakeBackfillGit::new(fixture());
    let commits = block_on(plan_commits(&options("f2", "f2"), &git)).unwrap();
    assert!(commits.iter().eq(std::iter::once(&"f2")), "{commits:?}");
}

#[test]
fn plan_rejects_an_unresolvable_endpoint() {
    let git = FakeBackfillGit::new(fixture());
    let error = block_on(plan_commits(&options("absent", "f2"), &git)).unwrap_err();
    assert!(error.find_source::<BackfillError>().is_some());
}

#[test]
fn plan_rejects_a_from_that_is_not_an_ancestor_of_to() {
    // f1 is on the feature side, not in master's first-parent ancestry.
    let git = FakeBackfillGit::new(fixture());
    let error = block_on(plan_commits(&options("f1", "c3"), &git)).unwrap_err();
    assert!(error.find_source::<BackfillError>().is_some());
}

#[test]
fn plan_maps_a_ref_resolution_failure() {
    let mut history = fixture();
    history.fail_resolve();
    let git = FakeBackfillGit::new(history);

    let error = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap_err();

    assert!(error.find_source::<ResolveRefFailedError>().is_some());
    assert!(error.find_source::<FirstParentWalkFailedError>().is_none());
    assert!(error.find_source::<io::Error>().is_some());
}

#[test]
fn plan_maps_a_first_parent_walk_failure() {
    let mut history = fixture();
    history.fail_first_parent();
    let git = FakeBackfillGit::new(history);

    let error = block_on(plan_commits(&options("c1", "f2"), &git)).unwrap_err();

    assert!(error.find_source::<FirstParentWalkFailedError>().is_some());
    assert!(error.find_source::<ResolveRefFailedError>().is_none());
    assert!(error.find_source::<io::Error>().is_some());
}

#[test]
fn plan_backfills_a_to_outside_the_current_branch_history() {
    // HEAD is at feature; c3 (master tip) is not part of feature's history,
    // yet a range built purely from --to's first-parent ancestry still plans.
    let git = FakeBackfillGit::new(fixture());
    let commits = block_on(plan_commits(&options("c0", "c3"), &git)).unwrap();
    assert!(
        commits.iter().eq(["c3", "c2", "c1", "c0"].iter()),
        "the range is derived from --to, independent of the checkout: {commits:?}"
    );
}
