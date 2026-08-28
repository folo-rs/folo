//! Version-group consistency and closure over members.

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};

use crate::fixture::{Fixture, write_package};
use crate::harness::check;

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn group_closure_with_member_absent_from_base_is_consistent() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    fixture.commit("alpha only");
    let base = fixture.sha("HEAD");
    write_package(&fixture, "beta", "0.1.0", "");
    fixture.commit("add group member absent from base");

    let (passed, message) = check(&fixture, &base);
    assert!(passed, "{message}");
}

/// A version group keeps released versions in lockstep, so a member that is
/// never published has no released version to keep in step. Accepting one would
/// leave it out of both the consistency verdict and the versions `apply`
/// rewrites, so the group's declared versions would silently drift apart.
#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn a_non_publishable_group_member_is_rejected() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    write_package(&fixture, "beta", "0.1.0", "\npublish = false");
    fixture.commit("group with a package that is never published");
    let base = fixture.sha("HEAD");

    let error = run(&RunInput::Check {
        base: Some(base),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        verbose: false,
    })
    .expect_err("a group cannot name a package that is never published");

    let message = error.to_string();
    assert!(message.contains("beta"), "{message}");
    assert!(message.contains("not a publishable package"), "{message}");
}

#[cfg_attr(miri, ignore)] // Spawns git and cargo, which Miri cannot emulate.
#[test]
fn inconsistent_group_fails_check_even_when_content_is_unchanged() {
    let fixture = Fixture::new(
        r#"
[workspace.metadata.release-plan.groups]
g = ["alpha", "beta"]
"#,
    );
    write_package(&fixture, "alpha", "0.1.0", "");
    write_package(&fixture, "beta", "0.2.0", "");
    fixture.commit("mismatched group versions");
    let base = fixture.sha("HEAD");

    let (passed, message) = check(&fixture, &base);
    assert!(!passed, "{message}");
    assert!(message.contains("inconsistent") || message.contains("different versions"));
    assert!(message.contains("increment-versions"));

    let outcome = run(&RunInput::Check {
        base: Some(base),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Github,
        verify_packaging: false,
        verbose: false,
    })
    .unwrap();
    match outcome {
        RunOutcome::Check { passed, message } => {
            assert!(!passed);
            assert!(message.contains("::error"));
        }
        other => panic!("expected check, got {other:?}"),
    }
}
