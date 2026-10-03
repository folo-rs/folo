//! Shared Git selections compared with the narrow native queries they replace.

#[cfg(unix)]
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::{ffi::OsStringExt as _, fs::PermissionsExt as _};

use crp_workspace::git::LiveObservations;
use crp_workspace::manifest::PathCase;

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(
    miri,
    ignore = "queries effective Git attributes and malformed native configuration"
)]
fn effective_filter_admission_is_scoped_and_does_not_confuse_special_driver_names() {
    let fixture = Repository::new();
    fixture.write("pkg/file", b"tracked");
    fixture.command(&["add", "pkg"]);
    let git = fixture.repo();
    for (attribute, selected) in [
        ("text eol=lf", false),
        ("filter=count", true),
        ("filter=unspecified", true),
        ("filter=unset", true),
        ("-filter", true),
        ("!filter", false),
    ] {
        fixture.write(
            ".git/info/attributes",
            format!("pkg/file {attribute}\noutside/file filter=count\n").as_bytes(),
        );
        assert_eq!(
            git.may_have_filter_drivers(&["pkg/file"]).unwrap(),
            selected
        );
    }
    fixture.write(".git/config", b"[invalid");
    git.may_have_filter_drivers(&["pkg/file"]).unwrap_err();
}

#[test]
#[cfg_attr(miri, ignore = "compares live Git pathspecs and index/worktree modes")]
fn shared_selections_equal_narrow_queries_for_literal_unicode_and_overlapping_paths() {
    let fixture = Repository::new();
    for path in [
        "Pkg/[literal]/file",
        "Pkg/[literal]/nested/Cargo.toml",
        "Pkg/plain/file",
        "Pkg2/file",
        "other/file",
        "shared/LICENSE",
        "\u{c4}rea/file",
        "Pkg/\u{df}/file",
    ] {
        fixture.write(path, b"tracked");
    }
    fixture.command(&["add", "Pkg", "Pkg2", "other", "shared", "\u{c4}rea"]);
    fixture.command(&[
        "update-index",
        "--chmod=+x",
        "shared/LICENSE",
        "Pkg/plain/file",
    ]);
    fixture.command(&["commit", "--quiet", "-m", "tracked scopes"]);
    fixture.write("Pkg/.gitignore", b"ignored\n");
    fixture.write("Pkg/ignored", b"ignored");
    for path in [
        "Pkg/[literal]/new",
        "Pkg/plain/new",
        "Pkg2/new",
        "other/new",
        "\u{c4}rea/new",
        "Pkg/\u{df}/new",
    ] {
        fixture.write(path, b"untracked");
    }
    fs::remove_file(fixture.path().join("Pkg/[literal]/nested/Cargo.toml")).unwrap();
    let git = fixture.repo();
    let tracked = git.ls_files("").unwrap();
    let directories = ["Pkg", "Pkg/[literal]", "Pkg/plain", "\u{c4}rea"];
    for case in [PathCase::Sensitive, PathCase::Insensitive] {
        let shared =
            LiveObservations::acquire(&git, &tracked, &directories, &["shared/LICENSE"], case)
                .unwrap();
        for scope in [
            "Pkg",
            "pkg",
            "Pkg/[literal]",
            "Pkg/plain",
            "Pkg/\u{df}",
            "\u{c4}rea",
            "\u{e4}rea",
            "other",
            "",
        ] {
            let paths = [scope, "shared/LICENSE"];
            assert_eq!(
                shared
                    .tracked_paths(&paths, || git.tracked_paths(&paths, case))
                    .unwrap(),
                git.tracked_paths(&paths, case).unwrap()
            );
            assert_eq!(
                shared
                    .modes(&paths, || git.work_tree_modes(&paths, case))
                    .unwrap(),
                git.work_tree_modes(&paths, case).unwrap()
            );
            assert_eq!(
                shared
                    .untracked_paths(scope, || git.ls_untracked(scope, case))
                    .unwrap(),
                git.ls_untracked(scope, case).unwrap()
            );
        }
    }
}

#[test]
#[cfg_attr(miri, ignore = "reacquires changed real Git index and untracked state")]
fn a_new_pass_observes_index_and_untracked_changes() {
    let fixture = Repository::new();
    fixture.write("pkg/file", b"one");
    fixture.command(&["add", "pkg"]);
    let git = fixture.repo();
    for iteration in 0..2 {
        let tracked = git.ls_files("").unwrap();
        let observed =
            LiveObservations::acquire(&git, &tracked, &["pkg"], &[], PathCase::Sensitive).unwrap();
        assert_eq!(
            observed
                .modes(&["pkg"], || panic!("shared"))
                .unwrap()
                .is_executable("pkg/file"),
            iteration == 1
        );
        assert_eq!(
            observed
                .untracked_paths("pkg", || panic!("shared"))
                .unwrap(),
            if iteration == 0 {
                vec![]
            } else {
                vec!["pkg/new"]
            }
        );
        assert_eq!(
            observed
                .tracked_paths(&["pkg"], || panic!("shared"))
                .unwrap()
                .len(),
            iteration + 1
        );
        fixture.command(&["update-index", "--chmod=+x", "pkg/file"]);
        fixture.write("pkg/added", b"added");
        fixture.command(&["add", "pkg/added"]);
        fixture.write("pkg/new", b"new");
    }
}

#[cfg(unix)]
#[test]
#[cfg_attr(
    miri,
    ignore = "uses Unix modes and an unrelated non-UTF8 untracked path"
)]
fn shared_scopes_preserve_worktree_mode_precedence_and_exclude_unrelated_errors() {
    let fixture = Repository::new();
    fixture.write("pkg/file", b"one");
    fixture.command(&["add", "pkg"]);
    fixture.command(&["update-index", "--chmod=+x", "pkg/file"]);
    fixture.command(&["commit", "--quiet", "-m", "index executable"]);
    fixture.command(&["config", "core.fileMode", "true"]);
    fs::set_permissions(
        fixture.path().join("pkg/file"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fs::create_dir_all(fixture.path().join("unrelated")).unwrap();
    fs::write(
        fixture
            .path()
            .join("unrelated")
            .join(OsString::from_vec(vec![0xff])),
        b"outside",
    )
    .unwrap();
    let git = fixture.repo();
    let tracked = git.ls_files("").unwrap();
    let shared =
        LiveObservations::acquire(&git, &tracked, &["pkg"], &[], PathCase::Sensitive).unwrap();
    assert!(
        !shared
            .modes(&["pkg"], || panic!("shared"))
            .unwrap()
            .is_executable("pkg/file")
    );
    assert!(
        shared
            .untracked_paths("pkg", || panic!("shared"))
            .unwrap()
            .is_empty()
    );
    git.ls_untracked("", PathCase::Sensitive).unwrap_err();
}
