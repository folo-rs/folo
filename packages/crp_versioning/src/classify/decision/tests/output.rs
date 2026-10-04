//! Fresh classification envelopes and decision diagnostic replay.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use crp_diag::Verbose;
use crp_workspace::git::testing::unopened;
use crp_workspace::manifest::{WorkspaceInherit, parse_package_manifest};
use crp_workspace::metadata::{DepKind, WorkPackage};

use super::fixture::{Recording, compute, inputs};
use crate::classify::decision::{DecisionCache, Decisions};
use crate::classify::{ChangedItem, PackageStatus, Verdict, fixture, log_evidence};

#[test]
fn payload_rebuilds_all_live_envelope_fields_and_dependency_kinds() {
    let input = inputs();
    let payload = serde_json::to_vec(&compute(&input).unwrap()).unwrap();
    let mut current = fixture::classification(Vec::new());
    current.head = "fresh-head".into();
    current.git = unopened(Path::new("moved-workspace"));
    current.work_tree.workspace_root = "moved-workspace".into();
    current.work_tree.tracked_paths = vec!["fresh-listing".into()];
    current.work_tree.packages.push(WorkPackage {
        manifest: parse_package_manifest(
            "[package]\nname='p'\nversion='1.0.0'\n",
            "p/Cargo.toml",
            &WorkspaceInherit::default(),
        )
        .unwrap()
        .unwrap(),
        manifest_path: Path::new("moved-workspace").join("p/Cargo.toml"),
        dependencies: vec![
            input
                .packages
                .first()
                .unwrap()
                .dependencies
                .first()
                .unwrap()
                .0
                .clone(),
        ],
        consumer_contract: true,
        has_lockfile_target: false,
        resources: BTreeMap::new(),
    });
    let decisions: Decisions = serde_json::from_slice(&payload).unwrap();
    decisions.apply(&input, &mut current);
    assert_eq!(current.head, "fresh-head");
    assert_eq!(current.git.root(), Path::new("moved-workspace"));
    assert_eq!(current.work_tree.tracked_paths, ["fresh-listing"]);
    assert_eq!(
        current.packages.first().unwrap().manifest_path,
        Path::new("moved-workspace").join("p/Cargo.toml")
    );
    assert_eq!(
        current
            .packages
            .first()
            .unwrap()
            .dependencies
            .first()
            .unwrap()
            .kind,
        DepKind::Build
    );
    assert_eq!(
        current.packages.first().unwrap().status(),
        PackageStatus::NeedsIncrement
    );
    assert!(
        !String::from_utf8(payload)
            .unwrap()
            .contains("moved-workspace")
    );
}

#[test]
fn diagnostics_distinguish_computation_from_memory_and_storage_reuse() {
    let input = inputs();
    let recording = Recording::default();
    let verbose = Verbose::new(true, &recording);
    let mut cache = DecisionCache::default();
    let value = cache
        .get_with(
            input.key().unwrap(),
            verbose,
            |_, call| call(),
            || compute(&input),
        )
        .unwrap();
    assert!(
        recording
            .0
            .lock()
            .unwrap()
            .contains("computed classification decisions")
    );
    recording.0.lock().unwrap().clear();
    cache
        .get_with(input.key().unwrap(), verbose, |_, _| panic!(), || panic!())
        .unwrap();
    assert!(recording.0.lock().unwrap().contains("from memory"));
    recording.0.lock().unwrap().clear();
    DecisionCache::default()
        .get_with(input.key().unwrap(), verbose, |_, _| Ok(value), || panic!())
        .unwrap();
    assert!(recording.0.lock().unwrap().contains("from storage"));

    let notes = RefCell::new(Vec::new());
    let mut class = fixture::package("p", PackageStatus::NeedsIncrement, "");
    let Verdict::NeedsIncrement { changed, .. } = &mut class.verdict else {
        panic!()
    };
    changed.push(ChangedItem::Lockfile {
        dependency: "locked-dependency".into(),
        change: "updated".into(),
    });
    log_evidence(&notes, &class);
    assert!(notes.borrow().iter().any(|line| line.contains("inherited")));
    assert!(
        notes
            .borrow()
            .iter()
            .any(|line| line.contains("locked-dependency") && line.contains("updated"))
    );
    assert!(notes.borrow().iter().any(|line| line.contains("status")));
}
