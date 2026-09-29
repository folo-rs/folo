//! External acquisition for apply.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use crp_diag::Verbose;
use crp_versioning::apply::*;
use crp_versioning::plan::{PlanFile, PlanIncrement, PlanStage, ResolvedVersions};
use semver::Version;
use tempfile::tempdir;
use toml_edit::DocumentMut;

fn v(text: &str) -> Version {
    text.parse().unwrap()
}

fn dep_item(toml: &str) -> DocumentMut {
    toml.parse::<DocumentMut>().unwrap()
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Reads plan artifacts and verifies that live files remain unchanged"
)]
fn apply_never_accepts_a_proposal_or_an_uncaptured_expansion() {
    let directory = tempdir().unwrap();
    let manifest = directory.path().join("Cargo.toml");
    let lockfile = directory.path().join("Cargo.lock");
    let plan_path = directory.path().join("plan.json");
    let original_manifest = "[package]\nname='demo'\nversion='1.0.0'\n";
    let original_lockfile = "# live lockfile\n";
    fs::write(&manifest, original_manifest).unwrap();
    fs::write(&lockfile, original_lockfile).unwrap();
    for stage in [PlanStage::Proposed, PlanStage::Expanded] {
        let plan = PlanFile::new(
            stage,
            vec![PlanIncrement {
                name: "demo".to_owned(),
                bump: None,
                version: Some("2.0.0".to_owned()),
            }],
        );
        fs::write(&plan_path, serde_json::to_vec(&plan).unwrap()).unwrap();
        for dry_run in [false, true] {
            _ = run_apply(
                &plan_path,
                dry_run,
                &manifest,
                Verbose::new(false, &crp_diag::Discard),
            )
            .unwrap_err();
            assert_eq!(fs::read_to_string(&manifest).unwrap(), original_manifest);
            assert_eq!(fs::read_to_string(&lockfile).unwrap(), original_lockfile);
        }
    }
}

/// A path outside the workspace keeps its own requirement.
///
/// A same-named package living outside the workspace is a different package, so its requirement
/// must survive a plan that names ours.
// The lexical form does not match, so the rewrite falls through to asking the filesystem, which
// Miri's isolation refuses.
#[cfg_attr(miri, ignore)]
#[test]
fn a_path_outside_the_workspace_keeps_its_own_requirement() {
    let resolved = ResolvedVersions {
        packages: BTreeMap::from([("demo".to_string(), v("0.2.0"))]),
    };
    let members = demo_members();
    let targets = targets_for("/ws/packages/caller", &members);

    let outside_text =
        "[dependencies]\ndemo = { version = \"0.1.0\", path = \"../../vendor/demo\" }\n";
    let mut outside = dep_item(outside_text);
    rewrite_dependency_tables(
        &mut outside,
        &targets,
        &resolved,
        Verbose::new(false, &crp_diag::Discard),
    );

    assert_eq!(outside.to_string(), outside_text);
}

/// A path that does not exist declares no member.
///
/// A path that reaches nothing on disk names no member, whatever its spelling, so an outside
/// dependency stays untouched.
#[cfg_attr(miri, ignore)] // tempdir is host filesystem, which Miri cannot emulate.
#[test]
fn a_path_that_does_not_exist_declares_no_member() {
    let dir = tempdir().unwrap();
    let members = BTreeMap::from([(dir.path().join("packages/demo"), "demo".to_string())]);
    let targets = DepTargets {
        manifest_dir: dir.path().join("packages/caller"),
        members_by_dir: &members,
    };

    assert!(!targets.declares("../gone", "demo"));
}

#[test]
#[cfg_attr(miri, ignore = "canonicalizes real filesystem directories")]
fn canonical_dependency_identity_requires_both_name_and_directory() {
    let directory = tempdir().unwrap();
    for name in ["member", "outside", "via"] {
        fs::create_dir_all(directory.path().join(name)).unwrap();
    }
    // An equivalent but non-lexical member spelling exercises filesystem identity acquisition.
    let members = BTreeMap::from([(directory.path().join("via/../member"), "member".to_string())]);
    let targets = DepTargets {
        manifest_dir: directory.path().to_path_buf(),
        members_by_dir: &members,
    };

    assert!(targets.declares("member", "member"));
    assert!(!targets.declares("member", "other"));
    assert!(!targets.declares("outside", "member"));
    assert!(!targets.declares("outside", "other"));
}

/// A workspace whose only member is `demo`.
///
/// It is laid out under a shared root so the rewrite tests can express both
/// in-workspace and outside paths.
fn demo_members() -> BTreeMap<PathBuf, String> {
    BTreeMap::from([(PathBuf::from("/ws/packages/demo"), "demo".to_string())])
}

fn targets_for<'a>(manifest_dir: &str, members: &'a BTreeMap<PathBuf, String>) -> DepTargets<'a> {
    DepTargets {
        manifest_dir: PathBuf::from(manifest_dir),
        members_by_dir: members,
    }
}
