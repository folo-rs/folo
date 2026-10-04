//! In-process builders and diagnostic recording for decision tests.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::rc::Rc;
use std::sync::Mutex;

use crp_diag::DiagnosticSink;
use crp_workspace::git::GitObjectContext;
use crp_workspace::inherited::InheritedKeys;
use crp_workspace::lockfile::{InstallationGraph, Lockfile};
use crp_workspace::manifest::PathCase;
use crp_workspace::manifest_document::ManifestDocument;
use crp_workspace::metadata::{DepKind, ReportedDep};
use ohno::AppError;
use semver::Version;
use toml_edit::DocumentMut;

use crate::anchor::Anchor;
use crate::classify::decision::{
    AnchorInputs, DecisionInputs, Decisions, LockInputs, PackageInputs,
};
use crate::classify::{ChangedFile, ReleasedFiles};
use crate::inherited::InheritedInputs;

/// In-process diagnostic destination, without process stream mutation.
#[derive(Debug, Default)]
pub(crate) struct Recording(pub(crate) Mutex<String>);

impl DiagnosticSink for Recording {
    fn write(&self, text: &str) -> io::Result<()> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
}

pub(crate) fn syntax(text: &str) -> ManifestDocument {
    ManifestDocument::from_document(&text.parse().unwrap())
}

pub(crate) fn inputs() -> DecisionInputs {
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

pub(crate) fn compute(input: &DecisionInputs) -> Result<Decisions, AppError> {
    input.compute(
        |ids| Ok(ids.iter().map(|id| id.len()).collect()),
        |ids| Ok(ids.iter().map(|id| id.as_bytes().to_vec()).collect()),
    )
}

pub(crate) fn package(input: &mut DecisionInputs) -> &mut PackageInputs {
    input.packages.first_mut().unwrap()
}

pub(crate) fn anchor(input: &mut DecisionInputs) -> &mut AnchorInputs {
    package(input).anchor.as_mut().unwrap()
}

pub(crate) fn file(input: &mut DecisionInputs) -> &mut ChangedFile {
    anchor(input).files.entries.first_mut().unwrap()
}

pub(crate) fn inherited() -> InheritedInputs {
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

pub(crate) fn lock(version: &str) -> LockInputs {
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

pub(crate) fn work_lock(input: &mut DecisionInputs) -> &mut LockInputs {
    anchor(input).new_lock = Some("work".into());
    input
        .locks
        .entry("work".into())
        .or_insert_with(|| lock("1.0.0"))
}
