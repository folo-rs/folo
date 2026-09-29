//! Tests of tracked metadata eligibility without invoking Cargo discovery.

use super::*;
use crate::git::testing::unopened;

#[test]
fn only_tracked_manifests_are_workspace_members() {
    let root = Path::new("workspace");
    let git = unopened(root);
    let tracked = TrackedMetadata {
        git: &git,
        workspace_root: root,
        paths: vec!["pkg/Cargo.toml".to_string()],
        case: PathCase::Sensitive,
    };
    assert!(tracked.contains_manifest(&root.join("pkg/Cargo.toml").to_string_lossy()));
    assert!(!tracked.contains_manifest(&root.join("untracked/Cargo.toml").to_string_lossy()));
    assert!(!tracked.contains_manifest("outside/Cargo.toml"));
}
