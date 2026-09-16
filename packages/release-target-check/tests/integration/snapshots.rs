use std::fs;
use std::process::Command;

use tempfile::TempDir;

use crate::fixture::Fixture;
use crate::scheduling::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn accepts_later_library_snapshot_without_generating_a_lockfile() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write(".github/workflows/release.yml", "name: updated fixture\n");
        fixture.git(&["rm", "Cargo.lock"]);
        let commit = fixture.commit("workflow maintenance without a library lockfile");
        fixture.write("target/ordinary-build-output", "ignored");
        let output = fixture
            .verifier(&commit, &commit)
            .args(["--package", "widget@1.0.0", "--verbose"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
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
        let output = fixture.verify(&commit, &commit, "widget@1.0.0");
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
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
        let output = fixture.verify(&old, &main, "widget@1.0.0");
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
        assert!(output.stderr.is_empty());
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
        assert!(
            !fixture
                .verify(&main, &main, "widget@1.0.0")
                .status
                .success()
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn rejects_dirty_source_at_executable_boundary() {
    with_io_test(|| {
        let fixture = Fixture::new();
        let commit = fixture.head();
        fixture.write("packages/widget/src/lib.rs", "pub fn value() -> u8 { 2 }\n");
        assert!(
            !fixture
                .verify(&commit, &commit, "widget@1.0.0")
                .status
                .success()
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes the verifier against an empty directory")]
fn reports_argument_errors_at_executable_boundary() {
    with_io_test(|| {
        // Parser branches belong to cli's unit tests. Here only the executable's error
        // reporting matters, so no Git repository or Cargo workspace needs to be constructed.
        let directory = TempDir::new().unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_release-target-check"))
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!output.stderr.is_empty());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git and Cargo against filesystem fixtures")]
fn missing_lockfile_is_not_generated_as_a_repair() {
    with_io_test(|| {
        let fixture = Fixture::new();
        fixture.write("packages/widget/src/main.rs", "fn main() {}\n");
        // Anchor the binary with its lockfile before removing only that file. Otherwise the
        // added binary source could reject verification independently of lockfile policy.
        fixture.write_package("1.1.0");
        fixture.commit("binary release");
        fixture.git(&["rm", "Cargo.lock"]);
        let commit = fixture.commit("missing lockfile");
        assert!(
            !fixture
                .verify(&commit, &commit, "widget@1.1.0")
                .status
                .success()
        );
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
        assert!(
            !fixture
                .verify(&candidate, &main, "widget@1.0.0")
                .status
                .success()
        );
    });
}
