//! Complete-input identity and actual decision-call admission without native resources.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::Path;
use std::sync::Mutex;

use crp_diag::{DiagnosticSink, Discard};
use crp_workspace::git::testing::unopened;
use crp_workspace::inherited::InheritedKeys;
use crp_workspace::manifest::{
    DependencyPatch, DependencySource, InstallationDependencies, InstallationDependency,
    WorkspaceInherit, installation_error, parse_package_manifest,
};
use crp_workspace::metadata::WorkPackage;
use serde_json::{Value, json};
use toml_edit::DocumentMut;

use super::*;
use crate::VersionRegressionError;
use crate::classify::{ChangedFile, PackageStatus, fixture, log_evidence};

fn syntax(text: &str) -> ManifestDocument {
    ManifestDocument::from_document(&text.parse().unwrap())
}

fn inputs() -> DecisionInputs {
    DecisionInputs {
        objects: GitObjectContext::default(),
        case: PathCase::Sensitive,
        prefix: String::new(),
        release_history: "history".into(),
        merge_target: None,
        versions: BTreeMap::from([("p".into(), Version::new(1, 0, 0))]),
        exact_dependencies: Vec::new(),
        exempt: BTreeSet::new(),
        locks: BTreeMap::new(),
        packages: vec![PackageInputs {
            name: "p".into(),
            version: Version::new(1, 0, 0),
            directory: "p".into(),
            manifest: syntax("[package]\nname='p'\nversion='1.0.0'\n"),
            dependencies: vec![(
                ReportedDep {
                    name: "dependency".into(),
                    req: "1".into(),
                    exact_pin: false,
                    kind: DepKind::Build,
                    public: false,
                },
                DepKind::Build,
            )],
            consumer_contract: true,
            resources: BTreeMap::new(),
            auto_readme: true,
            untracked: Vec::new(),
            anchor: Some(AnchorInputs {
                anchor: Anchor {
                    commit: "anchor".into(),
                    version: Version::new(1, 0, 0),
                },
                files: ReleasedFiles {
                    entries: vec![ChangedFile {
                        path: "src/lib.rs".into(),
                        old_id: Some("old".into()),
                        new_id: Some("new".into()),
                        old_mode: "100644",
                        new_mode: "100644",
                        mode_change: None,
                    }],
                },
                inherited: InheritedInputs::default(),
                old_lock: None,
                new_lock: None,
            }),
        }],
    }
}

fn compute(input: &DecisionInputs) -> Result<Decisions, AppError> {
    input.compute(
        |ids| Ok(ids.iter().map(|id| id.len()).collect()),
        |ids| Ok(ids.iter().map(|id| id.as_bytes().to_vec()).collect()),
    )
}

fn package(input: &mut DecisionInputs) -> &mut PackageInputs {
    input.packages.first_mut().unwrap()
}

fn anchor(input: &mut DecisionInputs) -> &mut AnchorInputs {
    package(input).anchor.as_mut().unwrap()
}

fn file(input: &mut DecisionInputs) -> &mut ChangedFile {
    anchor(input).files.entries.first_mut().unwrap()
}

fn inherited() -> InheritedInputs {
    InheritedInputs::acquire(
        &InheritedKeys {
            package: vec!["description".into()],
            ..InheritedKeys::default()
        },
        &DocumentMut::new(),
        &"[workspace.package]\ndescription='changed'"
            .parse()
            .unwrap(),
    )
}

fn lock(version: &str) -> LockInputs {
    let lockfile = Lockfile::parse(
        &format!(
            "version=4\n[[package]]\nname='p'\nversion='1.0.0'\ndependencies=['d']\n\
         [[package]]\nname='d'\nversion='{version}'\n"
        ),
        "lock",
    )
    .unwrap();
    LockInputs {
        lockfile: Rc::new(lockfile),
        installation: InstallationGraph::default(),
    }
}

fn work_lock(input: &mut DecisionInputs) -> &mut LockInputs {
    anchor(input).new_lock = Some("work".into());
    input
        .locks
        .entry("work".into())
        .or_insert_with(|| lock("1.0.0"))
}

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
    let base = inputs();
    let key = base.key().unwrap().unwrap();
    let mut changed = base.clone();
    change(&mut changed);
    assert_ne!(changed.key().unwrap().as_ref(), Some(&key));
    let mut cache = DecisionCache {
        last: Some((key, compute(&base).unwrap())),
    };
    let called = Cell::new(false);
    let actual = cache
        .get_with(
            changed.key().unwrap(),
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
fn lock_graph_and_installation_interpretation_are_inputs_before_closure_computation() {
    let mut input = inputs();
    work_lock(&mut input);
    let key = input.key().unwrap();
    let mut changed = input.clone();
    let lock = work_lock(&mut changed);
    Rc::make_mut(&mut lock.lockfile)
        .entries
        .get_mut(1)
        .unwrap()
        .version = Version::new(1, 1, 0);
    assert_ne!(key, changed.key().unwrap());
    let result = compute(&changed).unwrap();
    let Verdict::NeedsIncrement { changed, .. } = &result.packages.get("p").unwrap().0 else {
        panic!()
    };
    assert!(changed.iter().any(|change| {
        matches!(change, ChangedItem::Lockfile { dependency, .. } if dependency == "d")
    }));

    for source in [
        DependencySource::NamedRegistry("custom".into()),
        DependencySource::Registry("https://registry.example/index".into()),
    ] {
        let mut changed = input.clone();
        let installation = &mut work_lock(&mut changed).installation;
        installation.insert(
            "p".into(),
            Version::new(1, 0, 0),
            vec![InstallationDependency {
                name: "d".into(),
                requirement: Some("1".parse().unwrap()),
                source,
            }],
        );
        assert_ne!(key, changed.key().unwrap());
    }
    let mut changed = input.clone();
    let installation = &mut work_lock(&mut changed).installation;
    installation
        .registries
        .insert("custom".into(), "https://index.example".into());
    assert_ne!(key, changed.key().unwrap());
    work_lock(&mut changed)
        .installation
        .patches
        .push(DependencyPatch {
            origin: "crates-io".into(),
            name: "d".into(),
            replacement: Ok(InstallationDependency {
                name: "d".into(),
                requirement: None,
                source: DependencySource::NamedRegistry("custom".into()),
            }),
        });
    assert_ne!(key, changed.key().unwrap());
}

#[test]
fn equal_inputs_skip_actual_computation_in_memory_and_in_a_new_invocation() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let first = cache
        .get_with(
            input.key().unwrap(),
            quiet,
            |_, call| call(),
            || compute(&input),
        )
        .unwrap();
    let memory = cache
        .get_with(input.key().unwrap(), quiet, |_, _| panic!(), || panic!())
        .unwrap();
    let persisted = serde_json::to_vec(&first).unwrap();
    let independent = DecisionCache::default()
        .get_with(
            input.key().unwrap(),
            quiet,
            |_, _| Ok(serde_json::from_slice(&persisted).unwrap()),
            || panic!(),
        )
        .unwrap();
    assert_eq!(
        serde_json::to_value(first).unwrap(),
        serde_json::to_value(memory).unwrap()
    );
    assert_eq!(serde_json::to_vec(&independent).unwrap(), persisted);
}

#[test]
fn deterministic_key_contains_producer_and_revision_and_keeps_dependency_kinds() {
    let mut first = inputs();
    work_lock(&mut first);
    let second = first.clone();
    assert_eq!(first.key().unwrap(), second.key().unwrap());
    let key: Value = serde_json::from_str(&first.key().unwrap().unwrap()).unwrap();
    assert_eq!(key.get(0).unwrap(), env!("CARGO_PKG_VERSION"));
    assert_eq!(key.get(1).unwrap(), Decisions::REVISION);
    assert_eq!(
        key.pointer("/2/packages/0/dependencies/0/1").unwrap(),
        "Build"
    );
    let mut incompatible = key.clone();
    *incompatible.get_mut(1).unwrap() = json!(Decisions::REVISION + 1);
    assert_ne!(
        serde_json::to_string(&key).unwrap(),
        serde_json::to_string(&incompatible).unwrap()
    );
}

#[test]
fn errors_and_disabled_storage_never_populate_successful_memory() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let error = cache
        .get_with(
            input.key().unwrap(),
            quiet,
            |_, call| call(),
            || Err(io::Error::other("decision failure").into()),
        )
        .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
    assert!(cache.last.is_none());
    cache
        .get(&input, &Cache::default(), quiet, || compute(&input))
        .unwrap();
    assert!(cache.last.is_none());
    let mut regression = input.clone();
    package(&mut regression).version = Version::new(0, 1, 0);
    let error = cache
        .get_with(
            regression.key().unwrap(),
            quiet,
            |_, call| call(),
            || compute(&regression),
        )
        .unwrap_err();
    assert!(error.find_source::<VersionRegressionError>().is_some());
    assert!(cache.last.is_none());
    cache
        .get_with(
            input.key().unwrap(),
            quiet,
            |_, call| call(),
            || compute(&input),
        )
        .unwrap();
    assert!(cache.last.is_some());
}

#[test]
fn immutable_rendering_errors_do_not_publish_decisions() {
    let input = inputs();
    let quiet = Verbose::new(false, &Discard);
    let mut cache = DecisionCache::default();
    let error = cache
        .get_with(
            input.key().unwrap(),
            quiet,
            |_, compute| compute(),
            || {
                input.compute(
                    |ids| Ok(vec![0; ids.len()]),
                    |_| Err(io::Error::other("object unavailable").into()),
                )
            },
        )
        .unwrap_err();
    assert!(error.find_source::<io::Error>().is_some());
    assert!(cache.last.is_none());
}

#[test]
fn deferred_errors_and_replacement_contexts_bypass_reuse_without_failing_unrelated_work() {
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
    assert!(input.key().unwrap().is_none());
    compute(&input).unwrap();
    let mut input = inputs();
    let mut context = serde_json::to_value(&input.objects).unwrap();
    *context.get_mut("replacements").unwrap() = "replacement".into();
    input.objects = serde_json::from_value(context).unwrap();
    assert!(input.key().unwrap().is_none());
}

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

/// In-process diagnostic destination, without process stream mutation.
#[derive(Debug, Default)]
struct Recording(Mutex<String>);

impl DiagnosticSink for Recording {
    fn write(&self, text: &str) -> io::Result<()> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
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
