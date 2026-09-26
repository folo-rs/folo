use std::fs;

use crate::candidate::fixture::Fixture;
use crate::candidate::scheduling::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn accepts_later_library_snapshot_without_generating_a_lockfile() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write(".github/workflows/release.yml", "name: updated fixture\n");
        fixture.git(&["rm", "Cargo.lock"]);
        let commit = fixture.commit("workflow maintenance without a library lockfile");
        fixture.write("target/ordinary-build-output", "ignored");
        assert!(
            !fixture
                .verify(&commit, &commit, "1.0.0")
                .unwrap()
                .is_empty()
        );
        assert_eq!(fixture.head(), commit);
        assert!(fixture.git(&["status", "--porcelain"]).is_empty());
        assert!(!fixture.root().join("Cargo.lock").exists());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn rejects_changed_inherited_value_using_the_release_checker() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write_workspace("Apache-2.0");
        let commit = fixture.commit("unreleased inherited license");
        fixture.verify(&commit, &commit, "1.0.0").unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn accepts_older_candidate_before_release_line_version_advances() {
    with_io_test(|| {
        let fixture = Fixture::new();
        let old = fixture.head();
        let lockfile = fs::read(fixture.root().join("Cargo.lock")).unwrap();
        fixture.write_package("1.1.0");
        fixture.write("packages/widget/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
        let main = fixture.commit("next release");
        fixture.git(&["checkout", "--detach", &old]);
        fixture.verify(&old, &main, "1.0.0").unwrap();
        assert_eq!(fixture.head(), old);
        assert!(fixture.git(&["status", "--porcelain"]).is_empty());
        assert_eq!(
            fs::read(fixture.root().join("Cargo.lock")).unwrap(),
            lockfile
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn rejects_requested_version_after_main_advances() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write_package("1.1.0");
        let main = fixture.commit("next release");
        fixture.verify(&main, &main, "1.0.0").unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn rejects_dirty_source_before_verification() {
    with_io_test(|| {
        let fixture = Fixture::new();
        let commit = fixture.head();
        fixture.write("packages/widget/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
        fixture.verify(&commit, &commit, "1.0.0").unwrap_err();
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn missing_lockfile_is_not_generated_as_a_repair() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write("packages/widget/src/main.rs", "fn main() {}\n");
        // Anchor the binary before removing only its lockfile, isolating lockfile policy.
        fixture.write_package("1.1.0");
        fixture.commit("binary release");
        fixture.git(&["rm", "Cargo.lock"]);
        let commit = fixture.commit("missing lockfile");
        fixture.verify(&commit, &commit, "1.1.0").unwrap_err();
        assert!(!fixture.root().join("Cargo.lock").exists());
        assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn rejects_side_branch_even_after_a_merge_to_main() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.git(&["checkout", "-b", "feature"]);
        fixture.write("feature-notes", "not released\n");
        let candidate = fixture.commit("feature work");
        fixture.git(&["checkout", "main"]);
        fixture.git(&["merge", "--no-ff", "feature", "-m", "merge feature"]);
        let main = fixture.head();
        fixture.git(&["checkout", "--detach", &candidate]);
        fixture.verify(&candidate, &main, "1.0.0").unwrap_err();
    });
}
