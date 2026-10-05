//! Complete acquired-input changes and deterministic decision identity.

use std::cell::Cell;
use std::io;
use std::rc::Rc;

use crp_diag::{Discard, Verbose};
use crp_workspace::cache::{CacheEntry, key_digest};
use crp_workspace::lockfile::InstallationGraph;
use crp_workspace::manifest::{
    DependencyPatch, DependencySource, InstallationDependencies, InstallationDependency, PathCase,
    installation_error,
};
use crp_workspace::metadata::DepKind;
use semver::Version;

use super::fixture::{anchor, compute, file, inherited, inputs, lock, package, syntax, work_lock};
use crate::classify::decision::{DecisionCache, DecisionInputs, Decisions};
use crate::classify::{ChangedItem, Verdict};

// Separate cases keep each interpreter workload bounded while exercising the complete matrix.
macro_rules! input_changes {
    ($($name:ident => $change:expr),+ $(,)?) => {
        $(#[test] fn $name() { assert_input_change($change); })+
    };
}

input_changes! {
        private_member => |i| {
            i.versions
                .insert("private-member".into(), Version::new(1, 0, 0));
        },
        member_version => |i| {
            i.versions.insert("p".into(), Version::new(1, 0, 1));
        },
        exact_edge => |i| {
            i.exact_dependencies.push((
                "p".into(),
                "dependency".into(),
                "=1.0.0".into(),
                "p/Cargo.toml".into(),
                "dependencies.d".into(),
            ));
        },
        group_exemption => |i| {
            i.exempt.insert("p".into());
        },
        package_version => |i| {
            package(i).version = Version::new(1, 0, 1);
        },
        dependency_requirement => |i| {
            package(i).dependencies.first_mut().unwrap().0.req = "2".into();
        },
        dependency_kind => |i| {
            package(i).dependencies.first_mut().unwrap().1 = DepKind::Dev;
        },
        public_dependency => |i| {
            package(i).dependencies.first_mut().unwrap().0.public = true;
        },
        consumer_contract => |i| {
            package(i).consumer_contract = false;
        },
        packaging_rules => |i| {
            package(i).manifest = syntax("[package]\ninclude=['src/**']");
        },
        package_directory => |i| {
            package(i).directory = "moved".into();
        },
        shared_resource => |i| {
            package(i)
                .resources
                .insert("README.md".into(), "shared.md".into());
        },
        automatic_readme => |i| {
            package(i).auto_readme = false;
        },
        untracked_advisory => |i| {
            package(i).untracked.push("advisory.rs".into());
        },
        archive_path => |i| {
            file(i).path = "nested/lib.rs".into();
        },
        cleaned_content => |i| {
            file(i).new_id = Some("changed".into());
        },
        absent_endpoint => |i| {
            file(i).old_id = None;
        },
        executable_mode => |i| {
            file(i).new_mode = "100755";
            file(i).mode_change = Some(("100644", "100755"));
        },
        released_membership => |i| {
            anchor(i).files.entries.clear();
        },
        inherited_fields => |i| {
            anchor(i).inherited = inherited();
        },
        anchor_lock_graph => |i| {
            i.locks.insert("anchor".into(), lock("1.0.0"));
            anchor(i).old_lock = Some("anchor".into());
        },
        worktree_lock_graph => |i| {
            *work_lock(i) = lock("1.1.0");
        },
        anchor_commit => |i| {
            anchor(i).anchor.commit = "different-anchor".into();
        },
        anchor_version => |i| {
            anchor(i).anchor.version = Version::new(0, 9, 0);
        },
        new_package => |i| {
            package(i).anchor = None;
        },
        release_history => |i| {
            i.release_history = "moved-history".into();
        },
        merge_target => |i| {
            i.merge_target = Some("anticipated-parent".into());
        },
        path_case => |i| {
            i.case = PathCase::Insensitive;
        },
        workspace_prefix => |i| {
            i.prefix = "nested-workspace".into();
        },
}

fn assert_input_change(change: impl FnOnce(&mut DecisionInputs)) {
    let quiet = Verbose::new(false, &Discard);
    let base = inputs();
    let key = base.key(quiet).unwrap().unwrap();
    let mut changed = base.clone();
    change(&mut changed);
    let changed_key = changed.key(quiet).unwrap();
    assert_ne!(changed_key.as_ref(), Some(&key));
    let mut cache = DecisionCache {
        last: Some((key, compute(&base).unwrap())),
    };
    let called = Cell::new(false);
    let actual = cache
        .get_with(
            changed_key,
            Verbose::new(false, &Discard),
            |_, compute| compute(),
            || {
                called.set(true);
                compute(&changed)
            },
        )
        .unwrap();
    assert!(called.get());
    assert_eq!(
        serde_json::to_value(actual).unwrap(),
        serde_json::to_value(compute(&changed).unwrap()).unwrap()
    );
}

#[test]
fn lock_graph_is_an_input_before_closure_computation() {
    let quiet = Verbose::new(false, &Discard);
    let mut input = inputs();
    work_lock(&mut input);
    let key = input.key(quiet).unwrap();
    let mut changed = input.clone();
    let lock = work_lock(&mut changed);
    Rc::make_mut(&mut lock.lockfile)
        .entries
        .get_mut(1)
        .unwrap()
        .version = Version::new(1, 1, 0);
    assert_ne!(key, changed.key(quiet).unwrap());
    let result = compute(&changed).unwrap();
    let Verdict::NeedsIncrement { changed, .. } = &result.packages.get("p").unwrap().0 else {
        panic!()
    };
    assert!(changed.iter().any(|change| {
        matches!(change, ChangedItem::Lockfile { dependency, .. } if dependency == "d")
    }));
}

fn assert_installation_input_change(change: impl FnOnce(&mut InstallationGraph)) {
    let quiet = Verbose::new(false, &Discard);
    let mut input = inputs();
    work_lock(&mut input);
    let key = input.key(quiet).unwrap();
    change(&mut work_lock(&mut input).installation);
    assert_ne!(key, input.key(quiet).unwrap());
}

fn assert_installation_source_input(source: DependencySource) {
    assert_installation_input_change(|installation| {
        installation.insert(
            "p".into(),
            Version::new(1, 0, 0),
            vec![InstallationDependency {
                name: "d".into(),
                requirement: Some("1".parse().unwrap()),
                source,
            }],
        );
    });
}

#[test]
fn installation_registry_name_is_an_input() {
    assert_installation_source_input(DependencySource::NamedRegistry("custom".into()));
}

#[test]
fn installation_registry_index_is_an_input() {
    assert_installation_source_input(DependencySource::Registry(
        "https://registry.example/index".into(),
    ));
}

#[test]
fn installation_registry_configuration_is_an_input() {
    assert_installation_input_change(|installation| {
        installation
            .registries
            .insert("custom".into(), "https://index.example".into());
    });
}

#[test]
fn installation_patch_is_an_input() {
    assert_installation_input_change(|installation| {
        installation.patches.push(DependencyPatch {
            origin: "crates-io".into(),
            name: "d".into(),
            replacement: Ok(InstallationDependency {
                name: "d".into(),
                requirement: None,
                source: DependencySource::NamedRegistry("custom".into()),
            }),
        });
    });
}

#[test]
fn deterministic_digest_keeps_dependency_kinds() {
    let quiet = Verbose::new(false, &Discard);
    let first = inputs();
    let second = first.clone();
    assert_eq!(first.key(quiet).unwrap(), second.key(quiet).unwrap());
    assert_eq!(
        serde_json::to_value(&first)
            .unwrap()
            .pointer("/packages/0/dependencies/0/1")
            .unwrap(),
        "Build"
    );
}

#[test]
fn digest_covers_the_complete_serialized_model() {
    let quiet = Verbose::new(false, &Discard);
    let input = inputs();
    assert_eq!(
        input.key(quiet).unwrap().unwrap(),
        key_digest(&(env!("CARGO_PKG_VERSION"), Decisions::REVISION, &input)).unwrap()
    );
}

#[test]
fn digest_covers_the_computation_revision() {
    let quiet = Verbose::new(false, &Discard);
    let input = inputs();
    assert_ne!(
        input.key(quiet).unwrap().unwrap(),
        key_digest(&(env!("CARGO_PKG_VERSION"), Decisions::REVISION + 1, &input)).unwrap()
    );
}

#[test]
fn digest_covers_the_producer() {
    let quiet = Verbose::new(false, &Discard);
    let input = inputs();
    assert_ne!(
        input.key(quiet).unwrap().unwrap(),
        key_digest(&("different-producer", Decisions::REVISION, &input)).unwrap()
    );
}

#[test]
fn deferred_errors_and_replacement_contexts_bypass_reuse_without_failing_unrelated_work() {
    let quiet = Verbose::new(false, &Discard);
    let mut input = inputs();
    let graph = &mut work_lock(&mut input).installation;
    graph.members.insert(
        "unrelated".into(),
        (
            Version::new(1, 0, 0),
            InstallationDependencies::Invalid(installation_error(
                io::Error::other("deferred").into(),
            )),
        ),
    );
    assert!(input.key(quiet).unwrap().is_none());
    compute(&input).unwrap();
    let mut input = inputs();
    let mut context = serde_json::to_value(&input.objects).unwrap();
    *context.get_mut("replacements").unwrap() = "replacement".into();
    input.objects = serde_json::from_value(context).unwrap();
    assert!(input.key(quiet).unwrap().is_none());
}
