//! In-process classification snapshots for report and check command orchestration.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use semver::Version;

use crate::anchor::Anchor;
use crate::classify::{ChangedItem, Classification, PackageClass, PackageStatus, Verdict};
use crate::git::testing::unopened;
use crate::groups::Groups;
use crate::lockfile::InstallationGraph;
use crate::manifest::PathCase;
use crate::metadata::{VersionTarget, WorkTree};

pub(crate) fn classification(packages: Vec<PackageClass>) -> Classification {
    let version_targets = packages
        .iter()
        .map(|package| VersionTarget {
            name: package.name.clone(),
            version: package.declared_version.clone(),
            manifest_path: package.manifest_path.clone(),
            publishable: true,
        })
        .collect();
    Classification {
        head: "classified-head".to_owned(),
        base: "release-base".to_owned(),
        packages,
        groups: BTreeMap::new(),
        work_tree: WorkTree {
            workspace_root: PathBuf::from("workspace"),
            packages: Vec::new(),
            version_targets,
            exact_dependencies: Vec::new(),
            member_manifests: Vec::new(),
            members_by_dir: BTreeMap::new(),
            groups: Groups::default(),
            installation: InstallationGraph::default(),
        },
        git: unopened(Path::new("workspace")),
        // No filesystem is consulted by these fixtures.
        case: PathCase::Sensitive,
    }
}

pub(crate) fn package(name: &str, status: PackageStatus, patch: &str) -> PackageClass {
    let anchor = Anchor {
        commit: "package-anchor".to_owned(),
        version: Version::new(1, 0, 0),
    };
    let changed = vec![if patch.is_empty() {
        ChangedItem::Inherited {
            field: "workspace.package.description".to_owned(),
        }
    } else {
        ChangedItem::Package {
            path: "src/lib.rs".to_owned(),
            change: "modified".to_owned(),
        }
    }];
    let version = if status == PackageStatus::PendingRelease {
        Version::new(1, 0, 1)
    } else {
        anchor.version.clone()
    };
    let verdict = match status {
        PackageStatus::Unchanged => {
            assert!(patch.is_empty());
            Verdict::Unchanged { anchor }
        }
        PackageStatus::NeedsIncrement => Verdict::NeedsIncrement {
            anchor,
            changed,
            patch: patch.to_owned(),
        },
        PackageStatus::PendingRelease => Verdict::PendingRelease {
            anchor,
            changed,
            patch: patch.to_owned(),
        },
    };
    let mut package = PackageClass::with_verdict(
        name,
        version,
        verdict,
        PathBuf::from(name).join("Cargo.toml"),
    );
    if !patch.is_empty() {
        package.stat.files = 1;
        package.stat.insertions = 1;
        package.stat.deletions = 1;
    }
    package
}
