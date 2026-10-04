//! Fresh classification envelopes and decision diagnostic replay.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crp_diag::Verbose;
use crp_workspace::cache::Cache;
use crp_workspace::git::testing::unopened;
use crp_workspace::manifest::{
    InstallationDependencies, WorkspaceInherit, installation_error, parse_package_manifest,
};
use crp_workspace::metadata::{DepKind, WorkPackage};
use semver::Version;
use serde_json::json;

use super::fixture::{Recording, compute, inputs, work_lock};
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
            input.key(verbose).unwrap(),
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
        .get_with(
            input.key(verbose).unwrap(),
            verbose,
            |_, _| panic!(),
            || panic!(),
        )
        .unwrap();
    assert!(recording.0.lock().unwrap().contains("from memory"));
    recording.0.lock().unwrap().clear();
    DecisionCache::default()
        .get_with(
            input.key(verbose).unwrap(),
            verbose,
            |_, _| Ok(value),
            || panic!(),
        )
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

#[test]
fn disabled_storage_bypass_diagnostic() {
    for enabled in [false, true] {
        let recording = Recording::default();
        let verbose = Verbose::new(enabled, &recording);
        let input = inputs();
        let mut cache = DecisionCache::default();
        let called = Cell::new(false);
        cache
            .get(&input, &Cache::default(), verbose, || {
                called.set(true);
                compute(&input)
            })
            .unwrap();
        assert!(called.get());
        assert!(cache.last.is_none());
        assert_eq!(
            recording.0.lock().unwrap().as_str(),
            if enabled {
                "[release-plan] computing classification decisions because no cache storage directory is enabled\n"
            } else {
                ""
            }
        );
    }
}

#[test]
fn replacement_bypass_diagnostic() {
    assert_bypass_diagnostic("replacements", true);
}

#[test]
fn graft_bypass_diagnostic() {
    assert_bypass_diagnostic("grafts", true);
}

#[test]
fn installation_bypass_diagnostic() {
    assert_bypass_diagnostic("installation", true);
}

#[test]
fn quiet_bypass_diagnostic() {
    assert_bypass_diagnostic("installation", false);
}

fn assert_bypass_diagnostic(context: &str, enabled: bool) {
    let recording = Recording::default();
    let verbose = Verbose::new(enabled, &recording);
    let mut input = inputs();
    let reason = if context == "installation" {
        work_lock(&mut input).installation.members.insert(
            "unrelated".into(),
            (
                Version::new(1, 0, 0),
                InstallationDependencies::Invalid(installation_error(
                    io::Error::other("deferred").into(),
                )),
            ),
        );
        "the acquired installation graph for lock endpoint \"work\" contains deferred errors"
    } else {
        let mut objects = serde_json::to_value(&input.objects).unwrap();
        *objects.get_mut(context).unwrap() = if context == "replacements" {
            json!("replacement")
        } else {
            json!(b"graft")
        };
        input.objects = serde_json::from_value(objects).unwrap();
        "the acquired Git object context contains replacement refs or grafts requiring fresh availability checks"
    };
    let key = input.key(verbose).unwrap();
    assert!(key.is_none());
    let mut cache = DecisionCache::default();
    let called = Cell::new(false);
    cache
        .get_with(
            key,
            verbose,
            |_, _| panic!("bypassed inputs must not load storage"),
            || {
                called.set(true);
                compute(&input)
            },
        )
        .unwrap();
    assert!(called.get());
    assert!(cache.last.is_none());
    assert_eq!(
        recording.0.lock().unwrap().as_str(),
        if enabled {
            format!("[release-plan] computing classification decisions because {reason}\n")
        } else {
            String::new()
        }
    );
}
