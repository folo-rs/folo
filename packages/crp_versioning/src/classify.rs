// Classification of publishable packages against their anchors.

#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns production code; child modules only organize unit tests."
)]

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::rc::Rc;
use std::{fs, io, mem, str};

use crp_diag::{NoteSink, Verbose, plural, quote_path, short_commit, short_type_name};
use crp_workspace::cache::{Cache, CacheOptions};
use crp_workspace::git::{
    CommitHeaders, GitObjectContext, GitRepo, HistoricalTree, TreeEntry, WorkTreeModes,
    decode_file, join_git_rel, tree_mode,
};
use crp_workspace::lockfile::{
    Closure, ClosureChange, InstallationGraph, Lockfile, closure_changes,
};
use crp_workspace::manifest::{
    DEFAULT_README_FILES, PackageIdentity, PackageManifest, PathCase, WorkspaceInherit,
    WorkspaceMembers, cargo_config_paths, collect_registry_indices, installation_error,
    installation_patches, is_workspace_excluded, is_workspace_member,
    package_manifest_from_document, parse_document, path_package_identity, to_git_separators,
    workspace_members_from_document,
};
#[cfg(test)]
use crp_workspace::manifest::{parse_package_manifest, parse_workspace_members};
use crp_workspace::manifest_document::ManifestDocuments;
use crp_workspace::metadata::{
    ReportedDep, WorkPackage, WorkTree, dependents_of, load_tracked_work_tree_with_documents,
};
use crp_workspace::packaging::{PackagingRules, relativize};
use ohno::AppError;
use semver::Version;
use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use toml_edit::DocumentMut;

use crate::anchor::{Anchor, Presence, TimelineEntry, anticipated_anchor, resolve_anchor};
use crate::diff::{FileVersion, file_diff, mode_change_diff};
use crate::groups::{GroupVerdict, Groups};
use crate::history::AssessmentHistory;
use crate::inherited::{InheritedChange, inherited_changes};
use crate::{
    LockfileClosureUnavailableError, MalformedLockfileError, ReadFileError, SymlinkReleasedError,
    VersionRegressionError,
};

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod fixture;

/// Name Cargo requires for a workspace lockfile.
const LOCKFILE_FILE_NAME: &str = "Cargo.lock";
/// Name Cargo requires for a package manifest.
pub const MANIFEST_FILE_NAME: &str = "Cargo.toml";

/// Workspace classification: every publishable package plus its group verdicts.
///
/// This is the result the `report` and `check` commands render.
#[derive(Debug)]
pub struct Classification {
    pub head: String,
    /// The caller's revision naming actual committed release history.
    ///
    /// Retained as the revision the caller named - or the configured default
    /// when it named none - rather than the commit it resolved to, so a
    /// diagnostic can quote a command that reproduces this run.
    pub release_history_revision: String,
    /// Actual committed release history resolved for this assessment.
    pub release_history: String,
    /// Distinct final parent identity; targets already in release history normalize to `None`.
    pub merge_target: Option<String>,
    pub packages: Vec<PackageClass>,
    pub groups: BTreeMap<String, GroupVerdict>,
    /// Membership derived for this same workspace state, reused by preview and report projection.
    pub membership: Groups,
    pub work_tree: WorkTree,
    pub git: GitRepo,
    /// The case rules probed for the volume hosting the work tree.
    ///
    /// Carried so a later packaging probe resolves paths exactly as
    /// classification did.
    pub case: PathCase,
}

/// Per-package classification: its status and the evidence behind it.
///
/// The status, anchor, and change evidence are not independent: a package that
/// does not exist in either assessment predecessor has no anchor and no evidence, while an
/// anchored package always has one and carries a patch whenever its released
/// files differ, whatever its status. The classifier produces only those
/// combinations, so they are held in one closed
/// [`Verdict`] rather than as separately writable fields. `check` gates the
/// process exit on the status while `report` emits the anchor and evidence
/// beside it, and the two must never disagree.
/// Ref: `packages/cargo-release-plan/docs/design.md`, "Package status".
#[derive(Clone, Debug)]
pub struct PackageClass {
    pub name: String,
    pub declared_version: Version,
    pub group: Option<String>,
    pub verdict: Verdict,
    pub stat: DiffStat,
    pub untracked: Vec<String>,
    pub dependencies: Vec<ReportedDep>,
    pub dependents: Vec<String>,
    /// Whether the package's library is documented for consumers.
    ///
    /// Ref: `crp_workspace::metadata::WorkPackage::consumer_contract`.
    pub consumer_contract: bool,
    pub manifest_path: PathBuf,
}

impl PackageClass {
    /// Classification status, as reported and as gated on.
    #[must_use]
    pub fn status(&self) -> PackageStatus {
        match &self.verdict {
            Verdict::New | Verdict::PendingRelease { .. } => PackageStatus::PendingRelease,
            Verdict::Unchanged { .. } => PackageStatus::Unchanged,
            Verdict::NeedsIncrement { .. } => PackageStatus::NeedsIncrement,
        }
    }

    /// The commit the released content was compared against, if there is one.
    #[must_use]
    pub fn anchor(&self) -> Option<&Anchor> {
        match &self.verdict {
            Verdict::New => None,
            Verdict::PendingRelease { anchor, .. }
            | Verdict::Unchanged { anchor }
            | Verdict::NeedsIncrement { anchor, .. } => Some(anchor),
        }
    }

    /// Released-content and inherited-value differences against the anchor.
    #[must_use]
    pub fn changed(&self) -> &[ChangedItem] {
        match &self.verdict {
            Verdict::New | Verdict::Unchanged { .. } => &[],
            Verdict::PendingRelease { changed, .. } | Verdict::NeedsIncrement { changed, .. } => {
                changed
            }
        }
    }

    /// The rendered file-difference patch.
    ///
    /// Empty when the package has no released file difference. Pending-release
    /// packages retain the patch because judging whether an existing increment
    /// covers accumulated changes needs the same evidence as choosing a new
    /// increment. Inherited workspace values and locked dependency identities are
    /// not file differences and never appear here.
    #[must_use]
    pub fn patch(&self) -> &str {
        match &self.verdict {
            Verdict::PendingRelease { patch, .. } | Verdict::NeedsIncrement { patch, .. } => patch,
            Verdict::New | Verdict::Unchanged { .. } => "",
        }
    }
}

/// Constructors for tests in other modules, which cannot name [`Verdict`].
///
/// They take only the evidence their outcome admits, so a test cannot assemble
/// a state the classifier would never produce. Everything the assertions do not
/// observe is left empty.
#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
impl PackageClass {
    pub(crate) fn unchanged(
        name: &str,
        declared_version: Version,
        anchor: Anchor,
        manifest_path: PathBuf,
    ) -> Self {
        Self::with_verdict(
            name,
            declared_version,
            Verdict::Unchanged { anchor },
            manifest_path,
        )
    }

    pub(crate) fn pending_release(
        name: &str,
        declared_version: Version,
        anchor: Anchor,
        manifest_path: PathBuf,
    ) -> Self {
        Self::with_verdict(
            name,
            declared_version,
            Verdict::PendingRelease {
                anchor,
                changed: Vec::new(),
                patch: String::new(),
            },
            manifest_path,
        )
    }

    pub(crate) fn needs_increment(
        name: &str,
        declared_version: Version,
        anchor: Anchor,
        changed: Vec<ChangedItem>,
        manifest_path: PathBuf,
    ) -> Self {
        Self::with_verdict(
            name,
            declared_version,
            Verdict::NeedsIncrement {
                anchor,
                changed,
                patch: String::new(),
            },
            manifest_path,
        )
    }

    /// A package created on this branch, with no anchor to compare against.
    pub(crate) fn new_package(
        name: &str,
        declared_version: Version,
        manifest_path: PathBuf,
    ) -> Self {
        Self::with_verdict(name, declared_version, Verdict::New, manifest_path)
    }

    fn with_verdict(
        name: &str,
        declared_version: Version,
        verdict: Verdict,
        manifest_path: PathBuf,
    ) -> Self {
        Self {
            name: name.to_string(),
            declared_version,
            group: None,
            verdict,
            stat: DiffStat {
                files: 0,
                insertions: 0,
                deletions: 0,
            },
            untracked: Vec::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            consumer_contract: true,
            manifest_path,
        }
    }
}

/// The classifier's outcome for one package, with the evidence it implies.
///
/// Each alternative carries exactly what that outcome can be justified by, so
/// there is no way to express an anchorless failure or an unchanged package that
/// still holds a patch. Ref: `packages/cargo-release-plan/docs/design.md`, "Package status".
#[derive(Clone, Debug)]
pub enum Verdict {
    /// The package was created on this branch.
    ///
    /// It is absent from release history and from its earlier first-parent
    /// history, so its creation counts as a version increase and there is
    /// nothing to compare against.
    New,
    /// The declared version increased over the anchor's.
    PendingRelease {
        anchor: Anchor,
        changed: Vec<ChangedItem>,
        patch: String,
    },
    /// The declared version did not increase, and neither did the content.
    Unchanged { anchor: Anchor },
    /// Released content differs from the anchor without a version increase.
    NeedsIncrement {
        anchor: Anchor,
        changed: Vec<ChangedItem>,
        patch: String,
    },
}

impl Verdict {
    /// Classifies acquired release evidence without accessing the repository.
    fn anchored(
        name: &str,
        declared: &Version,
        anchor: Anchor,
        changed: Vec<ChangedItem>,
        patch: String,
    ) -> Result<Self, AppError> {
        // The release anchor bounds every declared version, independently of content changes.
        // Ref: packages/cargo-release-plan/docs/design.md, "Version monotonicity".
        if *declared < anchor.version {
            return Err(VersionRegressionError::new(
                name,
                declared.clone(),
                anchor.version.clone(),
                &anchor.commit,
            )
            .into());
        }

        let status =
            PackageStatus::from_evidence(declared, Some(&anchor.version), !changed.is_empty())
                .expect("the anchor is present and version regression was rejected");
        Ok(match status {
            PackageStatus::PendingRelease => Self::PendingRelease {
                anchor,
                changed,
                patch,
            },
            PackageStatus::Unchanged => Self::Unchanged { anchor },
            PackageStatus::NeedsIncrement => Self::NeedsIncrement {
                anchor,
                changed,
                patch,
            },
        })
    }
}

/// Classification status of one publishable package.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PackageStatus {
    PendingRelease,
    NeedsIncrement,
    Unchanged,
}

impl PackageStatus {
    /// Derives the status shared by classification and report validation.
    ///
    /// Missing anchors have no comparison evidence; anchored versions must not regress.
    pub(crate) fn from_evidence(
        declared: &Version,
        anchor: Option<&Version>,
        has_changes: bool,
    ) -> Option<Self> {
        match anchor {
            None if has_changes => None,
            None => Some(Self::PendingRelease),
            Some(anchor) => match declared.cmp(anchor) {
                Ordering::Less => None,
                Ordering::Greater => Some(Self::PendingRelease),
                Ordering::Equal if has_changes => Some(Self::NeedsIncrement),
                Ordering::Equal => Some(Self::Unchanged),
            },
        }
    }
}

/// A released-content, inherited-value or locked-dependency change.
///
/// Serialized as the report.json object with `path`/`change`, `field`, or
/// `dependency`/`change`, plus `source`, so callers keep a stable JSON shape
/// without optional nulls.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "source", rename_all = "kebab-case")]
pub enum ChangedItem {
    Package { path: String, change: String },
    Inherited { field: String },
    Lockfile { dependency: String, change: String },
}

impl Serialize for ChangedItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The struct name reaches self-describing formats, so derive it from the
        // type rather than repeating its spelling in a literal.
        let name = short_type_name::<Self>();
        match self {
            Self::Package { path, change } => {
                let mut state = serializer.serialize_struct(name, 3)?;
                state.serialize_field("path", path)?;
                state.serialize_field("change", change)?;
                state.serialize_field("source", "package")?;
                state.end()
            }
            Self::Inherited { field } => {
                let mut state = serializer.serialize_struct(name, 2)?;
                state.serialize_field("field", field)?;
                state.serialize_field("source", "inherited")?;
                state.end()
            }
            Self::Lockfile { dependency, change } => {
                let mut state = serializer.serialize_struct(name, 3)?;
                state.serialize_field("dependency", dependency)?;
                state.serialize_field("change", change)?;
                state.serialize_field("source", "lockfile")?;
                state.end()
            }
        }
    }
}

/// Insertion/deletion counts for one package in `report.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DiffStat {
    pub files: usize,
    pub insertions: usize,
    pub deletions: usize,
}

/// Anchor identity as serialized in `report.json`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AnchorJson {
    pub commit: String,
    pub version: String,
}

pub fn classify(
    manifest_path: &Path,
    release_history: Option<&str>,
    verbose: Verbose<'_>,
) -> Result<Classification, AppError> {
    classify_with_target(manifest_path, release_history, None, verbose)
}

/// Assesses the working snapshot against actual history and an optional final parent snapshot.
pub fn classify_with_target(
    manifest_path: &Path,
    release_history: Option<&str>,
    merge_target: Option<&str>,
    verbose: Verbose<'_>,
) -> Result<Classification, AppError> {
    classify_with_cache(
        manifest_path,
        release_history,
        merge_target,
        verbose,
        &mut SnapshotCache::new(Cache::resolve(
            manifest_path,
            &CacheOptions::Default,
            verbose,
        )?),
    )
}

/// Reacquires candidate observations while reusing only context-bound committed snapshots.
pub fn classify_with_cache(
    manifest_path: &Path,
    release_history: Option<&str>,
    merge_target: Option<&str>,
    verbose: Verbose<'_>,
    cache: &mut SnapshotCache,
) -> Result<Classification, AppError> {
    let (mut work_tree, git) =
        load_tracked_work_tree_with_documents(manifest_path, &mut cache.documents, verbose)?;
    let history = AssessmentHistory::resolve(&git, release_history, merge_target, verbose)?;
    let release_history_revision = history.release_history_revision.clone();
    for package in &mut work_tree.packages {
        package.manifest.directory = join_git_rel(git.prefix(), &package.manifest.directory);
        package.resources =
            resolve_resources(&package.manifest, &package.manifest.directory, git.prefix());
    }
    let head = git.head()?;
    let history_commit = &history.release_history;
    verbose.note(|| {
        format!(
            "classifying {} against release history {} ({history_commit}); \
         anchors are the last parsed version change on that revision's first-parent line, \
         not on the work tree's branch",
            plural(work_tree.packages.len(), "publishable package"),
            quote_path(&release_history_revision)
        )
    });

    cache.bind_objects(GitObjectContext::capture(&git)?);
    cache.bind(
        &git,
        &work_tree.workspace_root,
        PathCase::probe(&work_tree.workspace_root),
        work_tree.installation.registries.clone(),
    );
    let history_snapshot = cache.snapshot(&git, history_commit, verbose)?;
    let target_snapshot = history
        .effective_target()
        .map(|target| cache.snapshot(&git, target, verbose))
        .transpose()?;
    let projected = target_snapshot.as_ref().map(|snapshot| {
        let target = history
            .effective_target()
            .expect("snapshot exists only for a distinct target");
        verbose.note(|| {
            format!(
                "merge target {target} supplies one final anticipated squash predecessor; \
             intermediate parent commits are not release history"
            )
        });
        (target, snapshot.as_ref())
    });
    let work_root_doc = work_tree.manifests.root(&work_tree.workspace_root);

    let commits = git.first_parent_manifest_commits(history_commit, cache.case())?;
    let mut classes = Vec::new();
    let groups = Groups::from_workspace(&work_tree);
    let versions = work_tree.target_versions();
    let exempt: HashSet<String> = work_tree
        .version_targets
        .iter()
        .filter(|target| {
            is_new_in_assessment(
                &history_snapshot,
                projected.map(|(_, snapshot)| snapshot),
                &target.name,
            )
        })
        .map(|target| target.name.clone())
        .collect();
    let mut lockfiles = LockfileCache {
        storage: cache.storage.clone(),
        verbose,
        case: cache.case(),
        work: None,
        anchors: mem::take(&mut cache.lockfiles),
    };

    for package in &work_tree.packages {
        let class = classify_one(
            package,
            &work_tree,
            &groups,
            &git,
            history_commit,
            &commits,
            &history_snapshot,
            projected,
            work_root_doc,
            cache,
            &mut lockfiles,
            verbose,
        )?;
        classes.push(class);
    }
    cache.lockfiles = lockfiles.anchors;

    let group_verdicts = groups.verdicts(&versions, &exempt);
    for (name, verdict) in &group_verdicts {
        let members: Vec<Cow<'_, str>> = verdict
            .members()
            .iter()
            .map(String::as_str)
            .map(quote_path)
            .collect();
        verbose.note(|| {
            format!(
                "version group {} is derived from exact workspace dependencies: members [{}]; \
                 consistent={} (members absent from the baseline are exempt from matching \
                 declared versions but remain version targets)",
                quote_path(name),
                members.join(", "),
                verdict.is_consistent()
            )
        });
    }

    history.verify(&git)?;
    Ok(Classification {
        head,
        release_history_revision,
        release_history: history.release_history.clone(),
        merge_target: history.merge_target.clone(),
        packages: classes,
        groups: group_verdicts,
        membership: groups,
        work_tree,
        git,
        case: cache.case(),
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "classification needs the work-tree package, both trees, the first-parent walk, and both snapshot caches together"
)]
fn classify_one(
    package: &WorkPackage,
    work_tree: &WorkTree,
    groups: &Groups,
    git: &GitRepo,
    history_commit: &str,
    commits: &[String],
    history_snapshot: &CommitSnapshot,
    projected: Option<(&str, &CommitSnapshot)>,
    work_root_doc: &DocumentMut,
    cache: &mut SnapshotCache,
    lockfiles: &mut LockfileCache<'_>,
    verbose: Verbose<'_>,
) -> Result<PackageClass, AppError> {
    let name = &package.manifest.name;
    // Diagnostics never render a repository-controlled name raw.
    // Ref: packages/cargo-release-plan/docs/implementation.md, "Diagnostics".
    let shown = quote_path(name);
    let group = groups.group_of(name).map(ToOwned::to_owned);
    let dependents = dependents_of(&work_tree.packages, name);

    let anticipated = projected.and_then(|(target, snapshot)| {
        anticipated_anchor(
            history_snapshot
                .packages
                .get(name)
                .map(|package| &package.version),
            snapshot.packages.get(name).map(|package| &package.version),
            target,
        )
    });
    let anchor = if let Some(anchor) = anticipated {
        anchor
    } else if history_snapshot.packages.contains_key(name) {
        let timeline = build_timeline(git, name, commits, cache, verbose)?;
        resolve_anchor(name, &timeline)?
    } else {
        // A package the baseline does not publish has no released version to
        // compare against, whether it never existed or once did and was
        // withdrawn. Reconciling a name that was published before is left to
        // whoever restores it: guessing which older release a restored
        // directory continues would make the tool's verdict depend on history
        // a fetch may not even carry.
        // Ref: packages/cargo-release-plan/docs/design.md, "Packages the baseline does not
        // publish".
        verbose.note(|| {
            format!(
                "{shown}: not published by the baseline {history_commit}, so it is treated as a new \
                 package whose first release this branch prepares"
            )
        });
        let side = work_tree_side(package, cache.case());
        let resource_paths: Vec<&str> = side.resources.values().map(String::as_str).collect();
        let tracked_paths = git.tracked_paths(&resource_paths, side.case)?;
        let tracked_resources = tracked_resources(&side, &tracked_paths);
        let content = released_in_work_tree(git, &side, &tracked_resources)?;
        let work_modes = work_tree_modes(git, &side, &tracked_resources)?;
        _ = validated_work_tree_files(git, name, &content.released, &work_modes)?;
        let untracked =
            untracked_released(git, &side, &tracked_resources, &content.present_tracked)?;
        log_untracked(&verbose, name, untracked.len());
        return Ok(PackageClass {
            name: name.clone(),
            declared_version: package.manifest.version.clone(),
            group,
            verdict: Verdict::New,
            stat: DiffStat {
                files: 0,
                insertions: 0,
                deletions: 0,
            },
            untracked,
            dependencies: package.dependencies.clone(),
            dependents,
            consumer_contract: package.consumer_contract,
            manifest_path: package.manifest_path.clone(),
        });
    };
    let short_anchor = short_commit(&anchor.commit);
    verbose.note(|| format!(
        "{shown}: anchor {short_anchor} declared {}; work tree declares {}; the package counts as \
         pending release only when the work-tree version is greater than the anchor version \
         (parsed, not textual)",
        anchor.version,
        package.manifest.version
    ));

    let anchor_snapshot = cache.snapshot(git, &anchor.commit, verbose)?;
    let anchor_pkg = anchor_snapshot
        .packages
        .get(name)
        .expect("the anchor commit is the newest commit at which the anchor version was observed, and both the timeline and this snapshot read that version from the same cache, so the package is present here");

    let (changed_files, patch, stat, untracked) = diff_package_with_tree(
        git,
        name,
        &anchor.commit,
        &PackageSide {
            dir: &anchor_pkg.directory,
            rules: &anchor_pkg.packaging,
            resources: &anchor_pkg.resources,
            auto_readme: anchor_pkg.auto_readme,
            case: cache.case(),
        },
        &work_tree_side(package, cache.case()),
        &anchor_snapshot.tree,
    )?;

    let mut changed = changed_files;
    let inherited: Vec<InheritedChange> = inherited_changes(
        &package.manifest.inherited,
        &anchor_snapshot.root_doc,
        work_root_doc,
    );
    for item in inherited {
        verbose.note(|| {
            format!(
                "{shown}: inherited {} changed between the anchor and the work tree, so the root \
             manifest is in scope for this package",
                quote_path(&item.field)
            )
        });
        changed.push(ChangedItem::Inherited { field: item.field });
    }

    log_untracked(&verbose, name, untracked.len());

    for (dependency, change) in lockfile_closure_changes(
        lockfiles,
        git,
        work_tree,
        name,
        anchor_pkg,
        package,
        &anchor.commit,
        &anchor_snapshot.installation,
    )? {
        verbose.note(|| {
            format!(
                "{shown}: the locked identity of {} is {} between the anchor and the work \
                 tree, and this package has an installable binary target at one or both \
                 endpoints, so the dependency is released content",
                quote_path(&dependency),
                change.as_str()
            )
        });
        changed.push(ChangedItem::Lockfile {
            dependency,
            change: change.as_str().to_owned(),
        });
    }

    let verdict = Verdict::anchored(name, &package.manifest.version, anchor, changed, patch)?;
    let class = PackageClass {
        name: name.clone(),
        declared_version: package.manifest.version.clone(),
        group,
        verdict,
        stat,
        untracked,
        dependencies: package.dependencies.clone(),
        dependents,
        consumer_contract: package.consumer_contract,
        manifest_path: package.manifest_path.clone(),
    };
    log_status(&verbose, &class);

    Ok(class)
}

/// Explains an anchored classification without recomputing its verdict.
fn log_status(notes: &impl NoteSink, class: &PackageClass) {
    notes.note(|| {
        let anchor = class
            .anchor()
            .expect("the final classification note follows successful anchor resolution");
        let version_increased = class.declared_version > anchor.version;
        format!(
            "{}: status {:?} because version_increased={version_increased} and \
         changed_items={}",
            quote_path(&class.name),
            class.status(),
            class.changed().len()
        )
    });
}

// Native snapshot and parent acquisition; build_timeline_with owns ordered observation and stopping.
#[cfg_attr(test, mutants::skip)]
fn build_timeline(
    git: &GitRepo,
    name: &str,
    commits: &[String],
    cache: &mut SnapshotCache,
    verbose: Verbose<'_>,
) -> Result<Vec<TimelineEntry>, AppError> {
    // Header memory has a separate lifetime from live boundary verdicts. Split ownership
    // for the callbacks and restore it even when snapshot or history acquisition fails.
    let mut headers = mem::take(&mut cache.headers);
    let storage = cache.storage.clone();
    let objects = cache.objects.clone();
    let result = build_timeline_with(
        commits,
        |commit| {
            let snapshot = cache.snapshot(git, commit, verbose)?;
            Ok(snapshot_presence(&snapshot, name))
        },
        |commit| {
            let parent = headers.has_parent(git, commit, &objects, &storage, verbose)?;
            git.parent_boundary_with_header(commit, parent)
        },
    );
    cache.headers = headers;
    result
}

fn snapshot_presence(snapshot: &CommitSnapshot, name: &str) -> Presence {
    snapshot.packages.get(name).map_or_else(
        || {
            if snapshot.unpublished.contains(name) {
                Presence::Unpublished
            } else {
                Presence::Absent
            }
        },
        |package| Presence::Published(package.version.clone()),
    )
}

fn build_timeline_with(
    commits: &[String],
    mut observe: impl FnMut(&str) -> Result<Presence, AppError>,
    mut parent_boundary: impl FnMut(&str) -> Result<bool, AppError>,
) -> Result<Vec<TimelineEntry>, AppError> {
    let mut timeline = Vec::with_capacity(commits.len());
    for (index, commit) in commits.iter().enumerate() {
        let presence = observe(commit)?;
        let is_last = index
            .checked_add(1)
            .is_some_and(|next| next == commits.len());
        let has_parent = if is_last {
            parent_boundary(commit)?
        } else {
            true
        };
        timeline.push(TimelineEntry {
            commit: commit.clone(),
            presence,
            has_parent,
        });
        // Stop once we have observed a version different from the history endpoint (the
        // resolver only needs the first change plus whether the last entry is
        // a true root). Keep going until a change appears so creation vs
        // shallow can be distinguished.
        if can_stop_timeline(&timeline) {
            break;
        }
    }
    Ok(timeline)
}

/// One end of a released-content comparison.
///
/// The anchor and the work tree are resolved independently, so each end carries
/// its own package directory and packaging rules.
#[derive(Debug)]
pub struct PackageSide<'a> {
    pub dir: &'a str,
    pub rules: &'a PackagingRules,
    /// Files Cargo packs because a manifest key names them.
    ///
    /// Keyed by the path each takes inside the package archive and valued by
    /// its git-root-relative path.
    pub resources: &'a BTreeMap<String, String>,
    /// Whether Cargo picks this package's README by probing its directory.
    pub auto_readme: bool,
    /// How the volume hosting the workspace resolves path case.
    ///
    /// This decides whether a tracked spelling answers a README candidate Cargo
    /// probes for.
    pub case: PathCase,
}

/// Resolves the files a manifest key names for packaging.
///
/// Cargo packs the file named by `readme` or `license-file` regardless of
/// `include` and `exclude`, and from outside `package_dir` if that is where it
/// lives. A package's released content is therefore neither confined to its own
/// directory nor fully described by its packaging rules: a workspace-level
/// README that several members inherit is released content for every one of
/// them, and so is a README the package's own `include` leaves out.
/// Ref: packages/cargo-release-plan/docs/design.md, "Released content".
///
/// A resource that already lives inside the package directory keeps its own
/// package-relative path, matching where Cargo puts it. `package_dir`,
/// `workspace_prefix`, and the resolved values are all git-root-relative.
fn resolve_resources(
    manifest: &PackageManifest,
    package_dir: &str,
    workspace_prefix: &str,
) -> BTreeMap<String, String> {
    let workspace_prefix = workspace_prefix.trim_end_matches('/');
    let mut resolved = BTreeMap::new();
    let declared = manifest
        .resource_paths
        .iter()
        .map(|relative| (package_dir, relative))
        .chain(
            manifest
                .inherited_resource_paths
                .iter()
                .map(|relative| (workspace_prefix, relative)),
        );
    for (base, relative) in declared {
        let Some(full) = join_relative(base, relative) else {
            continue;
        };
        // Cargo leaves a resource that is already inside the package where it
        // is and only flattens one from outside into the crate root. Both are
        // recorded, because `include` and `exclude` do not apply to either: a
        // README the package excludes is still released content.
        // Cargo uses lexical containment for this archive name, even when the
        // filesystem resolves a differently cased directory to the same entry.
        let key = match relativize(&full, package_dir) {
            Some(rel) => rel.to_string(),
            // A resource from outside is flattened into the crate root under its
            // file name, which is everything after the last separator.
            None => full
                .rsplit_once('/')
                .map_or(full.as_str(), |(_, name)| name)
                .to_string(),
        };
        resolved.insert(key, full);
    }
    resolved
}

// Native acquisition is integration-tested; PackageDiff renders the acquired endpoint observations.
#[cfg_attr(test, mutants::skip)]
pub fn diff_package(
    git: &GitRepo,
    name: &str,
    anchor_commit: &str,
    anchor: &PackageSide<'_>,
    work_side: &PackageSide<'_>,
) -> Result<(Vec<ChangedItem>, String, DiffStat, Vec<String>), AppError> {
    let tree = HistoricalTree::new(git.ls_tree(anchor_commit, &[])?);
    diff_package_with_tree(git, name, anchor_commit, anchor, work_side, &tree)
}

#[cfg_attr(test, mutants::skip)] // Native current-input acquisition and blob reads.
fn diff_package_with_tree(
    git: &GitRepo,
    name: &str,
    anchor_commit: &str,
    anchor: &PackageSide<'_>,
    work_side: &PackageSide<'_>,
    anchor_tree: &HistoricalTree,
) -> Result<(Vec<ChangedItem>, String, DiffStat, Vec<String>), AppError> {
    // Released content is defined from git-tracked files, and a manifest
    // resource may sit outside the package directory or outside its packaging
    // rules, so the directory listing does not cover it. Querying Git for those
    // paths keeps an untracked README from being read off disk and reported as
    // a content change. Ref: packages/cargo-release-plan/docs/design.md, "Released content".
    let resource_paths: Vec<&str> = work_side.resources.values().map(String::as_str).collect();
    let tracked_paths = git.tracked_paths(&resource_paths, work_side.case)?;
    let tracked_resources = tracked_resources(work_side, &tracked_paths);

    let anchor_files = released_at_commit(anchor_tree, anchor);
    let work = released_in_work_tree(git, work_side, &tracked_resources)?;
    let work_files = &work.released;

    reject_anchor_symlinks(name, anchor_tree, &anchor_files)?;

    // Git converts content on its way into the object database, so a file on
    // disk and the blob recording it need not hold the same bytes. Comparing
    // content identity rather than raw bytes puts both ends in the one
    // representation Git itself compares by, which is what keeps an LFS-tracked
    // asset or a line-ending rule from making an untouched package look
    // changed. Ref: packages/cargo-release-plan/docs/implementation.md, "Classification".
    let work_modes = work_tree_modes(git, work_side, &tracked_resources)?;
    let work_ids = work_blob_ids(git, name, work_files, &work_modes)?;

    let (changed, patch, stat) = PackageDiff {
        anchor_files: &anchor_files,
        work_files,
        anchor_tree,
        work_modes: &work_modes,
        work_ids: &work_ids,
    }
    .render(
        |path| git.show_file_bytes(anchor_commit, path),
        |id| git.show_blob_bytes(id),
    )?;
    let untracked = untracked_released(git, work_side, &tracked_resources, &work.present_tracked)?;
    Ok((changed, patch, stat, untracked))
}

/// Acquired endpoint identities for one package's released-content comparison.
///
/// Content bytes are requested only for differing objects, after presence and mode decisions.
/// Ref: packages/cargo-release-plan/docs/implementation.md, "Classification".
struct PackageDiff<'a> {
    anchor_files: &'a HashMap<String, String>,
    work_files: &'a HashMap<String, String>,
    anchor_tree: &'a HistoricalTree,
    work_modes: &'a WorkTreeModes,
    work_ids: &'a HashMap<String, String>,
}

impl PackageDiff<'_> {
    fn render(
        &self,
        mut old_bytes: impl FnMut(&str) -> Result<Option<Vec<u8>>, AppError>,
        mut new_bytes: impl FnMut(&str) -> Result<Vec<u8>, AppError>,
    ) -> Result<(Vec<ChangedItem>, String, DiffStat), AppError> {
        let Self {
            anchor_files,
            work_files,
            anchor_tree,
            work_modes,
            work_ids,
        } = self;
        // Cargo copies the executable bit into the archive, so a file made
        // executable without an edit is released content that changed even though
        // its blob is untouched. Ref: packages/cargo-release-plan/docs/design.md, "Released content".
        let rels: BTreeSet<&str> = anchor_files
            .keys()
            .chain(work_files.keys())
            .map(String::as_str)
            .collect();

        let mut changed = Vec::new();
        let mut patch = String::new();
        let mut insertions = 0_usize;
        let mut deletions = 0_usize;

        for rel in rels {
            let old_id = anchor_files
                .get(rel)
                .and_then(|path| anchor_tree.entry(path).map(|entry| entry.id.as_str()));
            let new_id = work_ids.get(rel).map(String::as_str);
            // The mode is only a change while the file exists at both ends: an
            // addition or a deletion is already reported by presence alone.
            let mode_change = match (anchor_files.get(rel), work_files.get(rel)) {
                (Some(old_path), Some(new_path)) if old_id.is_some() && new_id.is_some() => {
                    let old_mode = tree_mode(
                        anchor_tree
                            .entry(old_path)
                            .is_some_and(TreeEntry::is_executable),
                    );
                    let new_mode = tree_mode(work_modes.is_executable(new_path));
                    (old_mode != new_mode).then_some((old_mode, new_mode))
                }
                _ => None,
            };
            if old_id == new_id && mode_change.is_none() {
                continue;
            }
            let kind = match (old_id.is_some(), new_id.is_some()) {
                (false, true) => "added",
                (true, false) => "deleted",
                _ => "modified",
            };
            changed.push(ChangedItem::Package {
                path: rel.to_string(),
                change: kind.to_string(),
            });
            if let Some((old_mode, new_mode)) = mode_change {
                patch.push_str(&mode_change_diff(rel, old_mode, new_mode).text);
            }
            // Equal object ids prove the bytes are unchanged. This check comes after
            // mode rendering so a mode-only binary change cannot gain a false
            // "Binary files differ" line from the content renderer.
            if old_id == new_id {
                continue;
            }
            // The content itself is only needed to render an identity change.
            let old = match anchor_files.get(rel).filter(|_| old_id.is_some()) {
                Some(path) => old_bytes(path)?,
                None => None,
            };
            let new = new_id.map(&mut new_bytes).transpose()?;
            let old_side = old.as_deref().map(|content| FileVersion {
                content,
                mode: tree_mode(anchor_files.get(rel).is_some_and(|path| {
                    anchor_tree
                        .entry(path)
                        .is_some_and(TreeEntry::is_executable)
                })),
            });
            let new_side = new.as_deref().map(|content| FileVersion {
                content,
                mode: tree_mode(
                    work_files
                        .get(rel)
                        .is_some_and(|path| work_modes.is_executable(path)),
                ),
            });
            let file_diff = file_diff(rel, old_side, new_side);
            insertions = insertions.saturating_add(file_diff.insertions);
            deletions = deletions.saturating_add(file_diff.deletions);
            patch.push_str(&file_diff.text);
        }

        let stat = DiffStat {
            files: changed.len(),
            insertions,
            deletions,
        };
        Ok((changed, patch, stat))
    }
}

/// Git modes for released work-tree paths.
///
/// The package directory does not cover a manifest resource that lives outside
/// it, so those paths are asked for alongside the directory. Only tracked
/// resources are asked for, because an untracked one is not released content
/// and Git records no mode for it.
// Native mode-query forwarding; resources outside the package are exercised by integration tests.
#[cfg_attr(test, mutants::skip)]
pub fn work_tree_modes(
    git: &GitRepo,
    side: &PackageSide<'_>,
    tracked_resources: &BTreeMap<String, String>,
) -> Result<WorkTreeModes, AppError> {
    let mut pathspecs = vec![side.dir];
    pathspecs.extend(tracked_resources.values().map(String::as_str));
    git.work_tree_modes(&pathspecs, side.case)
}

/// Object ids the released work-tree files would be stored under.
///
/// Keyed by the path each takes inside the package archive.
///
/// A tracked path the work tree no longer holds is left out, which is what makes
/// it read as deleted. A symbolic link stops the run here rather than being
/// hashed, because Git would hash the file it points at while the tree records
/// the link itself.
// Validates native files before hashing; the injected pairing operation is covered in process.
#[cfg_attr(test, mutants::skip)]
fn work_blob_ids(
    git: &GitRepo,
    name: &str,
    released: &HashMap<String, String>,
    modes: &WorkTreeModes,
) -> Result<HashMap<String, String>, AppError> {
    let files = validated_work_tree_files(git, name, released, modes)?;
    work_blob_ids_with(files, |paths| git.hash_objects(paths))
}

fn work_blob_ids_with(
    files: Vec<(&str, &str)>,
    hash: impl FnOnce(&[&str]) -> Result<Vec<String>, AppError>,
) -> Result<HashMap<String, String>, AppError> {
    let paths: Vec<&str> = files.iter().map(|(_, path)| *path).collect();
    let ids = hash(&paths)?;
    Ok(files
        .into_iter()
        .map(|(rel, _)| rel.to_string())
        .zip(ids)
        .collect())
}

/// Released work-tree files that are present and safe to hash.
///
/// Symbolic links stop classification before Git can dereference them. Both
/// index modes and filesystem metadata are required because either one may be
/// the only place that identifies a link in the current checkout.
#[expect(
    clippy::implicit_hasher,
    reason = "This internal acquisition operation consumes the classifier's concrete released-file map."
)]
// The in-process helper consumes the symlink observation; this adapter only acquires it.
#[cfg_attr(test, mutants::skip)]
pub fn validated_work_tree_files<'a>(
    git: &GitRepo,
    name: &str,
    released: &'a HashMap<String, String>,
    modes: &WorkTreeModes,
) -> Result<Vec<(&'a str, &'a str)>, AppError> {
    validated_work_tree_files_with(git.root(), name, released, modes, |path| {
        fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_symlink())
    })
}

/// Validates acquired link metadata independently of host symlink privileges.
fn validated_work_tree_files_with<'a>(
    root: &Path,
    name: &str,
    released: &'a HashMap<String, String>,
    modes: &WorkTreeModes,
    mut symlink_metadata: impl FnMut(&Path) -> io::Result<bool>,
) -> Result<Vec<(&'a str, &'a str)>, AppError> {
    let mut files = Vec::with_capacity(released.len());
    for (rel, path) in released {
        if modes.is_symlink(path) {
            return Err(SymlinkReleasedError::new(name, path).into());
        }
        match symlink_metadata(&root.join(path)) {
            Ok(true) => {
                return Err(SymlinkReleasedError::new(name, path).into());
            }
            Ok(false) => {}
            Err(error) if is_not_found(&error) => continue,
            Err(error) => {
                return Err(ReadFileError::caused_by(root.join(path), error).into());
            }
        }
        files.push((rel.as_str(), path.as_str()));
    }
    Ok(files)
}

/// Stops when the anchor released a symbolic link.
///
/// The tree's modes are the only place the distinction survives, so the paths
/// released at the anchor are matched against the links the tree records.
fn reject_anchor_symlinks(
    name: &str,
    tree: &HistoricalTree,
    released: &HashMap<String, String>,
) -> Result<(), AppError> {
    // The released paths are a hash map, so the lowest matching path is chosen
    // to keep the reported one stable across runs.
    let offender = released
        .values()
        .filter(|path| tree.entry(path).is_some_and(TreeEntry::is_symlink))
        .min()
        .cloned();
    match offender {
        Some(path) => Err(SymlinkReleasedError::new(name, &path).into()),
        None => Ok(()),
    }
}

fn released_at_commit(tree: &HistoricalTree, side: &PackageSide<'_>) -> HashMap<String, String> {
    let mut released = released_by_rules(tree.paths(), tree.paths(), side);
    if side.auto_readme
        && let Some((name, path)) =
            detected_readme_with(side.dir, side.case, |candidate| match side.case {
                PathCase::Sensitive => tree.entry(candidate).map(|entry| entry.path.clone()),
                PathCase::Insensitive => tree
                    .paths()
                    .iter()
                    .filter(|path| side.case.same_path(path, candidate))
                    .min()
                    .cloned(),
            })
    {
        released.entry(name).or_insert(path);
    }
    let resources = tracked_resources(side, tree.paths());
    // Reading a resource back from the commit yields nothing when the commit
    // did not track it, so the tree itself performs the tracked-only filter the
    // work tree needs `tracked_paths` for.
    add_resources(&mut released, resources.iter());
    released
}

/// Adds the files Cargo packs because a manifest key names them.
///
/// The packaging rules are not consulted: Cargo packs these regardless of
/// `include` and `exclude`. An entry never displaces a file the directory
/// listing already claimed at that path, matching Cargo, which keeps the first
/// claim on a path and warns rather than overwriting it.
fn add_resources<'a>(
    released: &mut HashMap<String, String>,
    resources: impl Iterator<Item = (&'a String, &'a String)>,
) {
    for (name, path) in resources {
        released.entry(name.clone()).or_insert_with(|| path.clone());
    }
}

/// Resolves manifest-declared resources to the spelling Git records.
///
/// Cargo follows the checkout's case rules when it opens the declared path.
/// Git's index and trees preserve their own spelling, which must be retained so
/// blob, mode, and historical lookups address the entry Git actually returned.
#[must_use]
pub fn tracked_resources(
    side: &PackageSide<'_>,
    tracked_paths: &[String],
) -> BTreeMap<String, String> {
    side.resources
        .iter()
        .filter_map(|(name, declared)| {
            tracked_paths
                .iter()
                .filter(|tracked| side.case.same_path(tracked, declared))
                .min()
                .map(|tracked| (name.clone(), tracked.clone()))
        })
        .collect()
}

/// Lists the untracked paths a package's rules would release.
///
/// Paths are package-relative.
///
/// These are advisory only: released content is defined from git-tracked files,
/// so an untracked path is never a change. Ref: packages/cargo-release-plan/docs/design.md,
/// "Released
/// content".
// Acquires Git listings and advisory resource presence for the pure selection operation.
#[cfg_attr(test, mutants::skip)]
pub fn untracked_released(
    git: &GitRepo,
    side: &PackageSide<'_>,
    tracked_resources: &BTreeMap<String, String>,
    tracked: &[String],
) -> Result<Vec<String>, AppError> {
    let listed: Vec<String> = git.ls_untracked(side.dir, side.case)?;
    Ok(untracked_released_with(
        side,
        tracked_resources,
        tracked,
        &listed,
        |path| git.root().join(path).symlink_metadata().is_ok(),
    ))
}

fn untracked_released_with(
    side: &PackageSide<'_>,
    tracked_resources: &BTreeMap<String, String>,
    tracked: &[String],
    listed: &[String],
    mut present: impl FnMut(&str) -> bool,
) -> Vec<String> {
    // The same nested-package boundary the tracked listing observes applies
    // here, or a file under a nested package would be advertised as content
    // Cargo would pack for the outer one. The manifest drawing that boundary
    // may itself still be untracked, so both listings feed the scan.
    let mut boundary_paths = listed.to_vec();
    boundary_paths.extend_from_slice(tracked);
    let nested = nested_package_dirs(&boundary_paths, side.dir, side.case);

    let mut untracked: Vec<String> = listed
        .iter()
        .filter(|full| !is_inside_any(full, &nested, side.case))
        .filter_map(|full| {
            let rel = side.case.relativize(full, side.dir)?.to_string();
            side.rules.is_released(&rel).then_some(rel)
        })
        .collect();
    // The listing above is filtered by the packaging rules and stops at the
    // package directory, so a resource that those rules exclude or that is
    // declared from outside would go unmentioned even though Cargo would pack
    // it. It is advisory in exactly the same way, and it is named by the path
    // it takes inside the package archive.
    untracked.extend(
        side.resources
            .iter()
            .filter(|&(name, path)| present(path) && !tracked_resources.contains_key(name))
            .map(|(name, _path)| name.clone()),
    );
    if side.auto_readme {
        // A README Cargo would detect is packed whatever the packaging rules
        // say. Detect across both sets because a higher-priority untracked name
        // can outrank a tracked fallback; only the selected untracked path is
        // advisory.
        let listed_set: HashSet<&str> = listed.iter().map(String::as_str).collect();
        let present: HashSet<&str> = tracked.iter().chain(listed).map(String::as_str).collect();
        if let Some((name, full)) = detected_readme(side.dir, &present, side.case)
            && listed_set.contains(full.as_str())
        {
            untracked.push(name);
        }
    }
    untracked.sort_unstable();
    untracked.dedup();
    untracked
}

/// The package-relative paths of one work-tree package's released content.
///
/// The packaging verifier compares Cargo's own listing against this, so it has
/// to be the very selection classification compares — reconstructing the set
/// from `include` and `exclude` alone would drop a README Cargo detects for
/// itself and take in the files of a nested package, reporting a mismatch on a
/// package whose rules are in fact right.
// Native packaging-probe adapter; shared released_from_paths selection is tested in process.
#[cfg_attr(test, mutants::skip)]
pub fn released_work_tree_paths(
    git: &GitRepo,
    package: &WorkPackage,
    case: PathCase,
) -> Result<BTreeSet<String>, AppError> {
    let side = work_tree_side(package, case);
    let resource_paths: Vec<&str> = side.resources.values().map(String::as_str).collect();
    let tracked_paths = git.tracked_paths(&resource_paths, side.case)?;
    let tracked_resources = tracked_resources(&side, &tracked_paths);
    let content = released_in_work_tree(git, &side, &tracked_resources)?;
    Ok(content.released.into_keys().collect())
}

/// The work-tree end of `package`'s released-content comparison.
#[must_use]
pub fn work_tree_side(package: &WorkPackage, case: PathCase) -> PackageSide<'_> {
    PackageSide {
        dir: &package.manifest.directory,
        rules: &package.manifest.packaging,
        resources: &package.resources,
        auto_readme: package.manifest.auto_readme,
        case,
    }
}

/// One work-tree package's released content and the listing it was drawn from.
///
/// The tracked listing is reused for the advisory untracked scan, which needs
/// the same nested-package boundary this selection observed.
struct WorkTreeContent {
    released: HashMap<String, String>,
    present_tracked: Vec<String>,
}

fn released_in_work_tree(
    git: &GitRepo,
    side: &PackageSide<'_>,
    tracked_resources: &BTreeMap<String, String>,
) -> Result<WorkTreeContent, AppError> {
    let tracked = git.tracked_paths(&[side.dir], side.case)?;
    let present = present_in_work_tree(git, &tracked)?;
    let mut released = released_from_paths(&tracked, &present, side);
    add_resources(&mut released, tracked_resources.iter());
    Ok(WorkTreeContent {
        released,
        present_tracked: present,
    })
}

/// Whether a group member is absent from an assessment predecessor.
///
/// Membership, not anchor presence, decides exemption from group matching. A reintroduced
/// package may retain an older anchor despite being absent from this snapshot. An unpublished
/// member remains present and bound to its group.
/// Ref: packages/cargo-release-plan/docs/design.md, "Version groups".
fn is_new_at_snapshot(snapshot: &CommitSnapshot, name: &str) -> bool {
    !snapshot.packages.contains_key(name) && !snapshot.unpublished.contains(name)
}

/// Only members absent from both predecessors are exempt from group version matching.
fn is_new_in_assessment(
    history: &CommitSnapshot,
    target: Option<&CommitSnapshot>,
    name: &str,
) -> bool {
    is_new_at_snapshot(history, name)
        && target.is_none_or(|snapshot| is_new_at_snapshot(snapshot, name))
}

/// The subset of `paths` the work tree still holds on disk.
///
/// Git lists a tracked file whose work-tree copy has been deleted, but Cargo
/// packages what is on disk: a nested manifest that is gone no longer stops
/// packing, and a deleted default README is no longer there to be found. The
/// tracked listing still decides eligibility — released content is defined from
/// git-tracked files — so this narrower listing is used only for structure.
/// A dangling symbolic link counts as present, because Git tracks the link
/// itself rather than what it points at.
///
/// An untracked nested manifest draws no boundary because untracked paths never
/// enter the release verdict. The optional packaging probe still reports when
/// Cargo would treat one as structural input under `--allow-dirty`.
/// Ref: packages/cargo-release-plan/docs/design.md, "Released content".
/// Only a path that is not there is absent; any other failure stops the run,
/// because reading it as a deletion would silently change what the package
/// releases.
// Native metadata adapter; presence, missing paths and operational errors are tested below.
#[cfg_attr(test, mutants::skip)]
pub fn present_in_work_tree(git: &GitRepo, paths: &[String]) -> Result<Vec<String>, AppError> {
    present_in_work_tree_with(git.root(), paths, |path| {
        fs::symlink_metadata(path).map(|_| ())
    })
}

fn present_in_work_tree_with(
    root: &Path,
    paths: &[String],
    mut metadata: impl FnMut(&Path) -> io::Result<()>,
) -> Result<Vec<String>, AppError> {
    let mut present = Vec::with_capacity(paths.len());
    for path in paths {
        let full = root.join(path);
        match metadata(&full) {
            Ok(()) => present.push(path.clone()),
            Err(error) if is_not_found(&error) => {}
            Err(error) => return Err(ReadFileError::caused_by(&full, error).into()),
        }
    }
    Ok(present)
}

/// Selects one package's released content from the tracked paths beneath it.
///
/// `tracked` decides eligibility and `present` decides structure: which
/// directories draw a nested package boundary, and whether a default README is
/// there to detect. The two coincide at a commit and differ in a work tree that
/// has deleted a tracked file.
///
/// Keys are package-relative, values are the git-root-relative paths the caller
/// reads the content back from.
fn released_from_paths(
    tracked: &[String],
    present: &[String],
    side: &PackageSide<'_>,
) -> HashMap<String, String> {
    let mut map = released_by_rules(tracked, present, side);
    if side.auto_readme {
        let present: HashSet<&str> = present.iter().map(String::as_str).collect();
        if let Some((name, full)) = detected_readme(side.dir, &present, side.case) {
            map.entry(name).or_insert(full);
        }
    }
    map
}

fn released_by_rules(
    tracked: &[String],
    present: &[String],
    side: &PackageSide<'_>,
) -> HashMap<String, String> {
    let nested = nested_package_dirs(present, side.dir, side.case);
    let mut map = HashMap::new();
    for full in tracked {
        if is_inside_any(full, &nested, side.case) {
            continue;
        }
        let Some(rel) = side.case.relativize(full, side.dir) else {
            continue;
        };
        if side.rules.is_released(rel) {
            map.insert(rel.to_string(), full.clone());
        }
    }
    map
}

/// The default README this end holds for `dir`.
///
/// Keyed by the name it takes in the package archive.
///
/// Cargo probes the package directory for its default names in order and packs
/// the first that exists without consulting `include` or `exclude`, so a package
/// that names no README still releases the one beside it. Detection runs over
/// the same tracked listing the rest of the comparison uses, because released
/// content is defined from git-tracked files. Cargo's probe goes through the
/// filesystem, so on a case-insensitive volume a tracked `readme.md` answers the
/// `README.md` candidate and the probed case rules decide the match; the key is
/// the tracked spelling, which keeps a re-spelling of the file visible as the
/// content change it is.
/// Ref: packages/cargo-release-plan/docs/design.md, "Released content".
fn detected_readme(dir: &str, present: &HashSet<&str>, case: PathCase) -> Option<(String, String)> {
    detected_readme_with(dir, case, |candidate| match case {
        PathCase::Sensitive => present.contains(candidate).then(|| candidate.to_string()),
        PathCase::Insensitive => present
            .iter()
            .filter(|held| case.same_path(held, candidate))
            .min()
            .map(|held| (*held).to_string()),
    })
}

fn detected_readme_with(
    dir: &str,
    case: PathCase,
    mut recorded: impl FnMut(&str) -> Option<String>,
) -> Option<(String, String)> {
    DEFAULT_README_FILES.iter().find_map(|name| {
        let candidate = join_relative(dir, name)?;
        let full = recorded(&candidate)?;
        let rel = case.relativize(&full, dir)?.to_string();
        Some((rel, full))
    })
}

/// Package directories nested strictly inside `dir`, read off the tracked paths.
///
/// `cargo package` stops at a nested package boundary: a directory beneath the
/// package that carries its own `Cargo.toml` contributes nothing to the outer
/// package's released content, and Cargo applies that regardless of workspace
/// membership or of an explicit `include`. Reading the boundaries off the
/// manifests tracked on the side being examined therefore matches Cargo, where
/// reading them off the member list would attribute the files of an excluded or
/// otherwise non-member nested package to the outer package and report changes it
/// will never release.
fn nested_package_dirs(paths: &[String], dir: &str, case: PathCase) -> Vec<String> {
    paths
        .iter()
        .filter_map(|path| {
            let (parent, _) = path.rsplit_once('/')?;
            case.is_manifest(path).then_some(parent)
        })
        .filter(|parent| !case.same_path(parent, dir) && case.relativize(parent, dir).is_some())
        .map(ToOwned::to_owned)
        .collect()
}

fn is_inside_any(path: &str, dirs: &[String], case: PathCase) -> bool {
    dirs.iter().any(|dir| case.relativize(path, dir).is_some())
}

/// Package facts reconstructed from a historical tree, keyed by package name.
#[derive(Clone, Debug)]
/// One publishable member as it existed at a historical commit.
pub struct HistoricalPackage {
    pub directory: String,
    pub version: Version,
    pub packaging: PackagingRules,
    /// Files Cargo packs because a manifest key names them.
    ///
    /// Keyed by the path each takes inside the package archive.
    pub resources: BTreeMap<String, String>,
    /// Whether Cargo picks this package's README by probing its directory.
    pub auto_readme: bool,
    /// Whether an installable binary makes this endpoint's closure relevant.
    pub has_lockfile_target: bool,
}

/// Workspace members and root manifest at one commit.
#[derive(Clone, Debug)]
struct CommitSnapshot {
    tree: Rc<HistoricalTree>,
    packages: BTreeMap<String, HistoricalPackage>,
    /// Members that declared `publish = false` at this commit.
    ///
    /// Nothing could be released from them, so they carry no packaging facts,
    /// but the anchor walk still has to tell "withdrawn here" from "absent
    /// here". Ref: packages/cargo-release-plan/docs/implementation.md, "Anchor and change set".
    unpublished: BTreeSet<String>,
    root_doc: DocumentMut,
    installation: InstallationGraph,
}

/// Operation-scoped committed snapshots, never candidate observations or verdicts.
///
/// Ref: docs/implementation.md, "Shared operation and tests".
#[derive(Debug, Default)]
pub struct SnapshotCache {
    inner: HashMap<String, Rc<CommitSnapshot>>,
    // Only committed identities survive a pass; candidate bytes must be reacquired.
    lockfiles: HashMap<String, Lockfile>,
    context: Option<SnapshotContext>,
    storage: Cache,
    objects: GitObjectContext,
    headers: CommitHeaders,
    documents: ManifestDocuments,
}

/// Repository and interpretation inputs under which a commit snapshot is reusable.
#[derive(Debug, Eq, PartialEq)]
struct SnapshotContext {
    root: PathBuf,
    prefix: String,
    workspace_root: PathBuf,
    case: PathCase,
    registries: BTreeMap<String, String>,
}

impl SnapshotCache {
    #[must_use]
    pub fn new(storage: Cache) -> Self {
        Self {
            documents: ManifestDocuments::new(storage.clone()),
            storage,
            ..Self::default()
        }
    }

    fn clear(&mut self) {
        self.inner.clear();
        self.lockfiles.clear();
        self.headers.clear();
        // Parsed syntax is keyed by complete content, independent of repository interpretation.
        // Keep it when rebinding so the freshly acquired work tree can share it with history.
    }

    fn bind_objects(&mut self, objects: GitObjectContext) {
        if self.objects != objects {
            self.clear();
            self.objects = objects;
        }
    }

    fn bind(
        &mut self,
        git: &GitRepo,
        workspace_root: &Path,
        case: PathCase,
        registries: BTreeMap<String, String>,
    ) {
        let context = SnapshotContext {
            root: git.root().to_path_buf(),
            prefix: git.prefix().to_owned(),
            workspace_root: workspace_root.to_path_buf(),
            case,
            registries,
        };
        if self.context.as_ref() != Some(&context) {
            self.clear();
            self.context = Some(context);
        }
    }

    /// The probed case rules, shared by member matching and README detection.
    fn case(&self) -> PathCase {
        self.context
            .as_ref()
            .expect("classification binds its observation context before using snapshots")
            .case
    }

    // Native acquisition only on a cache miss; snapshot_with owns reuse and error handling.
    #[cfg_attr(test, mutants::skip)]
    fn snapshot(
        &mut self,
        git: &GitRepo,
        commit: &str,
        verbose: Verbose<'_>,
    ) -> Result<Rc<CommitSnapshot>, AppError> {
        let storage = self.storage.clone();
        let objects = self.objects.clone();
        let mut documents = mem::take(&mut self.documents);
        let result = self.snapshot_with(commit, |context| {
            let tree = HistoricalTree::load(git, commit, &objects, &storage, verbose)?;
            load_snapshot(
                git,
                commit,
                context.case,
                &context.registries,
                &Rc::new(tree),
                &mut documents,
                verbose,
            )
        });
        self.documents = documents;
        result
    }

    fn snapshot_with(
        &mut self,
        commit: &str,
        load: impl FnOnce(&SnapshotContext) -> Result<CommitSnapshot, AppError>,
    ) -> Result<Rc<CommitSnapshot>, AppError> {
        if let Some(existing) = self.inner.get(commit) {
            return Ok(Rc::clone(existing));
        }
        let context = self
            .context
            .as_ref()
            .expect("classification binds its observation context before using snapshots");
        let built = Rc::new(load(context)?);
        self.inner.insert(commit.to_string(), Rc::clone(&built));
        Ok(built)
    }
}

// Git acquisitions are native; load_snapshot_with reconstructs members from captured observations.
#[cfg_attr(test, mutants::skip)]
fn load_snapshot(
    git: &GitRepo,
    commit: &str,
    case: PathCase,
    registries: &BTreeMap<String, String>,
    tree: &Rc<HistoricalTree>,
    documents: &mut ManifestDocuments,
    verbose: Verbose<'_>,
) -> Result<CommitSnapshot, AppError> {
    let manifests: Vec<_> = tree
        .entries()
        .iter()
        // Gitlinks are not blobs, even when a submodule directory is named Cargo.toml.
        .filter(|entry| {
            case.is_manifest(&entry.path) && (entry.mode.starts_with("100") || entry.is_symlink())
        })
        .collect();
    let ids: Vec<_> = manifests.iter().map(|entry| entry.id.as_str()).collect();
    let blobs = git.show_blob_batch(&ids)?;
    let blobs: HashMap<_, _> = manifests
        .iter()
        .map(|entry| entry.path.as_str())
        .zip(blobs)
        .collect();
    load_snapshot_documents(git, case, registries, Rc::clone(tree), |path| {
        let content = match blobs.get(path) {
            // Decoding stays lazy: unrelated manifests need not be valid TOML or UTF-8.
            Some(bytes) => decode_file(Some(bytes.clone()), commit, path),
            None => git.show_file(commit, path),
        }?;
        content
            .map(|content| {
                if case.is_manifest(path) {
                    documents.parse(Path::new(path), &content, verbose)
                } else {
                    // Cargo configuration can contain credentials; it is interpreted freshly.
                    parse_document(Path::new(path), &content)
                }
            })
            .transpose()
    })
}

#[cfg(test)]
fn load_snapshot_with(
    git: &GitRepo,
    case: PathCase,
    registries: &BTreeMap<String, String>,
    tree: Rc<HistoricalTree>,
    mut read: impl FnMut(&str) -> Result<Option<String>, AppError>,
) -> Result<CommitSnapshot, AppError> {
    load_snapshot_documents(git, case, registries, tree, |path| {
        read(path)?
            .map(|content| parse_document(Path::new(path), &content))
            .transpose()
    })
}

fn load_snapshot_documents(
    git: &GitRepo,
    case: PathCase,
    registries: &BTreeMap<String, String>,
    tree: Rc<HistoricalTree>,
    mut read: impl FnMut(&str) -> Result<Option<DocumentMut>, AppError>,
) -> Result<CommitSnapshot, AppError> {
    let tree_paths = tree.paths();
    let requested_root = root_manifest_rel(git);
    let root_rel = case.recorded_path(tree_paths, &requested_root);
    // History before the workspace existed has no root manifest. An empty document has no
    // members or inheritance, so every current package is absent from that snapshot.
    let root_doc = match root_rel {
        Some(path) => read(path)?,
        None => None,
    }
    .unwrap_or_default();
    let members = workspace_members_from_document(&root_doc, case)?;
    // `members` globs are written relative to the workspace root, which need not be
    // the git root, while Git yields git-root-relative paths. Rebase before
    // matching, or a nested workspace would find no members and silently classify
    // every package as absent from the history endpoint.
    let workspace_prefix = git.prefix();
    let workspace = WorkspaceInherit::from_root(&root_doc);

    let mut manifest_paths: BTreeMap<String, String> = BTreeMap::new();
    for path in tree_paths.iter().filter(|path| case.is_manifest(path)) {
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let member_dir = workspace_relative_dir(dir, workspace_prefix, case);
        manifest_paths.insert(member_dir, path.clone());
    }

    let mut manifests = GitManifestSource {
        read: &mut read,
        workspace_prefix,
        workspace,
        paths: manifest_paths,
        parsed: BTreeMap::new(),
        case,
    };
    let member_dirs = resolve_members(&mut manifests, &members)?;
    let mut packages = BTreeMap::new();
    let mut unpublished = BTreeSet::new();
    let mut installation = InstallationGraph::default();
    let mut path_identities = BTreeMap::new();
    for member_dir in &member_dirs {
        let Some(parsed) = manifests.manifest(member_dir)? else {
            continue;
        };
        installation.insert(
            parsed.name.clone(),
            parsed.version.clone(),
            parsed.installation_dependencies.clone(),
        );
        path_identities.insert(
            join_git_rel(&parsed.directory, MANIFEST_FILE_NAME),
            Some(parsed.identity()),
        );
        if !parsed.publish {
            unpublished.insert(parsed.name.clone());
            continue;
        }
        packages.insert(
            parsed.name.clone(),
            HistoricalPackage {
                directory: parsed.directory.clone(),
                version: parsed.version.clone(),
                packaging: parsed.packaging.clone(),
                resources: resolve_resources(parsed, &parsed.directory, workspace_prefix),
                auto_readme: parsed.auto_readme,
                has_lockfile_target: parsed.targets.has_lockfile_target(
                    tree_paths
                        .iter()
                        .filter_map(|path| case.relativize(path, &parsed.directory)),
                    case,
                ),
            },
        );
    }
    if packages.values().any(|package| package.has_lockfile_target) {
        match historical_registries_documents(git.prefix(), registries, tree_paths, case, &mut read)
        {
            Ok(registries) => installation.registries = registries,
            Err(error) => installation.registry_error = Some(installation_error(error)),
        }
        installation.patches = installation_patches(&root_doc);
        resolve_historical_installation_documents(
            &mut installation,
            path_identities,
            git,
            tree_paths,
            case,
            &mut read,
        );
    }
    Ok(CommitSnapshot {
        tree,
        packages,
        unpublished,
        root_doc,
        installation,
    })
}

pub fn resolve_historical_installation_paths_with(
    installation: &mut InstallationGraph,
    identities: BTreeMap<String, Option<PackageIdentity>>,
    git: &GitRepo,
    tree_paths: &[String],
    case: PathCase,
    mut read: impl FnMut(&str) -> Result<Option<String>, AppError>,
) {
    resolve_historical_installation_documents(
        installation,
        identities,
        git,
        tree_paths,
        case,
        |path| {
            read(path)?
                .map(|content| parse_document(Path::new(path), &content))
                .transpose()
        },
    );
}

fn resolve_historical_installation_documents(
    installation: &mut InstallationGraph,
    mut identities: BTreeMap<String, Option<PackageIdentity>>,
    git: &GitRepo,
    tree_paths: &[String],
    case: PathCase,
    mut read: impl FnMut(&str) -> Result<Option<DocumentMut>, AppError>,
) {
    let mut documents = BTreeMap::<String, Option<DocumentMut>>::new();
    installation.resolve_paths(|reference| {
        let Some(directory) = reference.directory(git.root(), git.prefix(), "") else {
            return Ok(None);
        };
        let path = join_git_rel(&directory, MANIFEST_FILE_NAME);
        if let Some((_, identity)) = identities
            .iter()
            .find(|(candidate, _)| case.same_path(candidate, &path))
        {
            return Ok(identity.clone());
        }
        let identity = path_package_identity(&path, case, |path| {
            let Some(path) = tree_paths
                .iter()
                .find(|candidate| case.same_path(candidate, path))
            else {
                return Ok(None);
            };
            if let Some(document) = documents.get(path) {
                return Ok(document.clone());
            }
            let document = read(path)?;
            documents.insert(path.clone(), document.clone());
            Ok(document)
        })?;
        identities.insert(path, identity.clone());
        Ok(identity)
    });
}

/// Reconstructs tracked registry configuration over Cargo's ambient configuration.
///
/// Cargo loads ancestor configurations from outermost to innermost and prefers
/// the extensionless filename when both names exist in the same directory.
pub fn historical_registries_with(
    prefix: &str,
    ambient: &BTreeMap<String, String>,
    tree_paths: &[String],
    case: PathCase,
    mut read: impl FnMut(&str) -> Result<Option<String>, AppError>,
) -> Result<BTreeMap<String, String>, AppError> {
    historical_registries_documents(prefix, ambient, tree_paths, case, |path| {
        read(path)?
            .map(|content| parse_document(Path::new(path), &content))
            .transpose()
    })
}

fn historical_registries_documents(
    prefix: &str,
    ambient: &BTreeMap<String, String>,
    tree_paths: &[String],
    case: PathCase,
    mut read: impl FnMut(&str) -> Result<Option<DocumentMut>, AppError>,
) -> Result<BTreeMap<String, String>, AppError> {
    let mut registries = ambient.clone();
    for candidates in cargo_config_paths(prefix) {
        for path in candidates {
            let Some(path) = case.recorded_path(tree_paths, &path) else {
                continue;
            };
            let Some(doc) = read(path)? else {
                continue;
            };
            collect_registry_indices(&doc, &mut registries);
            break;
        }
    }
    Ok(registries)
}

/// Supplies historical member manifests to membership resolution.
///
/// Cargo loads only the manifests that can become members of the selected
/// workspace, so resolution pulls manifests through this abstraction instead of
/// parsing every `Cargo.toml` in the tree up front. An unrelated or excluded
/// nested workspace holding a manifest Cargo would never read must not be able
/// to fail classification. Ref: packages/cargo-release-plan/docs/implementation.md,
/// "Classification".
pub trait ManifestSource {
    /// Workspace-relative directories that hold a manifest, none of them parsed yet.
    fn candidate_dirs(&self) -> Vec<String>;

    /// The parsed manifest in `dir`, or `None` when `dir` holds no package.
    fn manifest(&mut self, dir: &str) -> Result<Option<&PackageManifest>, AppError>;

    /// Directories the path dependencies of the manifest in `dir` point at.
    ///
    /// Locally declared paths resolve against the member's own directory;
    /// inherited ones are declared in `[workspace.dependencies]` and so resolve
    /// against the workspace root. A path climbing above the workspace root
    /// names nothing inside it and is dropped. The result is owned rather than
    /// borrowed because following an edge parses further manifests through this
    /// same source.
    fn path_edges(&mut self, dir: &str) -> Result<Vec<String>, AppError> {
        let Some(parsed) = self.manifest(dir)? else {
            return Ok(Vec::new());
        };
        Ok(parsed
            .path_dependencies
            .iter()
            .map(|relative| join_relative(dir, relative))
            .chain(
                parsed
                    .inherited_path_dependencies
                    .iter()
                    .map(|relative| join_relative("", relative)),
            )
            .flatten()
            .collect())
    }
}

/// Reads member manifests out of one commit, parsing each at most once.
#[derive(Debug)]
pub struct GitManifestSource<'a, F> {
    /// Reads a recorded path from the selected commit.
    pub read: F,
    pub workspace_prefix: &'a str,
    pub workspace: WorkspaceInherit<'a>,
    /// Git-root-relative manifest path of every candidate directory.
    pub paths: BTreeMap<String, String>,
    pub parsed: BTreeMap<String, Option<PackageManifest>>,
    pub case: PathCase,
}

impl<F: FnMut(&str) -> Result<Option<DocumentMut>, AppError>> ManifestSource
    for GitManifestSource<'_, F>
{
    fn candidate_dirs(&self) -> Vec<String> {
        self.paths.keys().cloned().collect()
    }

    fn manifest(&mut self, dir: &str) -> Result<Option<&PackageManifest>, AppError> {
        let Some(dir) = self
            .paths
            .keys()
            .find(|candidate| self.case.same_path(candidate, dir))
            .cloned()
        else {
            return Ok(None);
        };
        if !self.parsed.contains_key(&dir) {
            let parsed = match self.paths.get(&dir) {
                Some(path) => match (self.read)(path)? {
                    Some(document) => {
                        package_manifest_from_document(&document, path, &self.workspace)?
                    }
                    None => None,
                },
                None => None,
            };
            self.parsed.insert(dir.clone(), parsed);
        }
        Ok(self.parsed.get(&dir).and_then(Option::as_ref))
    }

    fn path_edges(&mut self, dir: &str) -> Result<Vec<String>, AppError> {
        let workspace_prefix = self.workspace_prefix.trim_end_matches('/');
        let Some(parsed) = self.manifest(dir)? else {
            return Ok(Vec::new());
        };
        let package_dir = parsed.directory.clone();
        let local = parsed.path_dependencies.clone();
        let inherited = parsed.inherited_path_dependencies.clone();
        Ok(local
            .iter()
            .map(|relative| join_relative(&package_dir, relative))
            .chain(
                inherited
                    .iter()
                    .map(|relative| join_relative(workspace_prefix, relative)),
            )
            .flatten()
            .map(|target| workspace_relative_dir(&target, workspace_prefix, self.case))
            // Cargo implicitly adds path dependencies only when they live below
            // the workspace root. Explicit member patterns add outside members.
            .filter(|target| target != ".." && !target.starts_with("../"))
            // Follow recorded directory keys so aliases and cycles resolve one member.
            .filter_map(|target| {
                self.paths
                    .keys()
                    .find(|candidate| self.case.same_path(candidate, &target))
                    .cloned()
            })
            .collect())
    }
}

/// Reconstructs the workspace-relative directories Cargo would treat as members.
///
/// Beyond the declared `members` patterns, Cargo makes every path dependency of
/// a member that lives inside the workspace a member too, so the closure is
/// followed until it stops growing. Membership derived this way still honours
/// `exclude`.
pub fn resolve_members(
    manifests: &mut dyn ManifestSource,
    members: &WorkspaceMembers,
) -> Result<BTreeSet<String>, AppError> {
    let mut resolved: BTreeSet<String> = manifests
        .candidate_dirs()
        .into_iter()
        .filter(|dir| is_workspace_member(dir, members))
        .collect();
    let mut pending: Vec<String> = resolved.iter().cloned().collect();
    while let Some(dir) = pending.pop() {
        for target in manifests.path_edges(&dir)? {
            if is_workspace_excluded(&target, members) {
                continue;
            }
            if manifests.manifest(&target)?.is_none() {
                continue;
            }
            if resolved.insert(target.clone()) {
                pending.push(target);
            }
        }
    }
    Ok(resolved)
}

/// Resolves a manifest-relative path against a workspace-relative directory.
///
/// Returns `None` when the path climbs above the workspace root, because such a
/// dependency is outside the workspace and therefore not an implicit member.
fn join_relative(base: &str, relative: &str) -> Option<String> {
    let relative = to_git_separators(relative, MAIN_SEPARATOR);
    let mut segments: Vec<&str> = if base.is_empty() {
        Vec::new()
    } else {
        base.split('/').collect()
    };
    for segment in relative.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            other => segments.push(other),
        }
    }
    Some(segments.join("/"))
}

/// Parsed lockfiles shared by binary package classifications.
///
/// The work-tree endpoint is common to every package, while packages that share
/// an anchor commit also share its historical endpoint. Retaining both avoids
/// reparsing workspace-sized lockfiles for every binary package.
/// Ref: packages/cargo-release-plan/docs/implementation.md, "Lockfile closures".
#[derive(Debug)]
pub struct LockfileCache<'a> {
    pub work: Option<Lockfile>,
    pub anchors: HashMap<String, Lockfile>,
    pub case: PathCase,
    pub storage: Cache,
    pub verbose: Verbose<'a>,
}

impl LockfileCache<'_> {
    // Acquires tracked bytes only on a cache miss; anchor_with tests cache identity and parsing.
    #[cfg_attr(test, mutants::skip)]
    pub fn anchor<'a>(
        &'a mut self,
        git: &GitRepo,
        name: &str,
        commit: &str,
        path: &str,
    ) -> Result<&'a Lockfile, AppError> {
        let case = self.case;
        self.anchor_with(name, commit, path, || {
            let paths = git.ls_tree_paths(commit)?;
            match case.recorded_path(&paths, path) {
                Some(recorded) => git.show_file_bytes(commit, recorded),
                None => Ok(None),
            }
        })
    }

    fn anchor_with(
        &mut self,
        name: &str,
        commit: &str,
        path: &str,
        read: impl FnOnce() -> Result<Option<Vec<u8>>, AppError>,
    ) -> Result<&Lockfile, AppError> {
        if !self.anchors.contains_key(commit) {
            let Some(bytes) = read()? else {
                return Err(LockfileClosureUnavailableError::new(
                    name,
                    "the anchor commit does not track a workspace Cargo.lock",
                )
                .into());
            };
            let lockfile = Lockfile::parse_cached(
                &decode_lockfile(bytes, path)?,
                path,
                &self.storage,
                self.verbose,
            )?;
            self.anchors.insert(commit.to_owned(), lockfile);
        }
        Ok(self
            .anchors
            .get(commit)
            .expect("the requested anchor lockfile was inserted above when absent"))
    }

    pub fn work<'a>(
        &'a mut self,
        work_tree: &WorkTree,
        name: &str,
        git_path: &str,
    ) -> Result<&'a Lockfile, AppError> {
        if self.work.is_none() {
            let work_path = work_tree.workspace_root.join(LOCKFILE_FILE_NAME);
            let Some(bytes) = read_optional_bytes(&work_path, name, git_path)? else {
                return Err(LockfileClosureUnavailableError::new(
                    name,
                    "the work tree has no workspace Cargo.lock; restore or refresh Cargo.lock",
                )
                .into());
            };
            self.work = Some(Lockfile::parse_cached(
                &decode_lockfile(bytes, git_path)?,
                git_path,
                &self.storage,
                self.verbose,
            )?);
        }
        Ok(self
            .work
            .as_ref()
            .expect("the work-tree lockfile was initialized above when absent"))
    }
}

/// Dependencies whose locked identity changed between the anchor and the work tree.
///
/// Each endpoint that has an installable binary target must have a lockfile that
/// resolves the package at its corresponding declared version. An endpoint without
/// that target releases no closure and therefore contributes an empty closure.
/// Ref: packages/cargo-release-plan/docs/design.md, "Relevant lockfile closures".
#[expect(
    clippy::too_many_arguments,
    reason = "each endpoint supplies its target, lockfile and installation declarations"
)]
pub fn lockfile_closure_changes(
    cache: &mut LockfileCache<'_>,
    git: &GitRepo,
    work_tree: &WorkTree,
    name: &str,
    anchor_package: &HistoricalPackage,
    work_package: &WorkPackage,
    anchor_commit: &str,
    anchor_installation: &InstallationGraph,
) -> Result<Vec<(String, ClosureChange)>, AppError> {
    let git_path = join_git_rel(git.prefix(), LOCKFILE_FILE_NAME);
    let anchor = if anchor_package.has_lockfile_target {
        let lockfile = cache.anchor(git, name, anchor_commit, &git_path)?;
        required_closure(
            lockfile.closure(
                name,
                &anchor_package.version.to_string(),
                anchor_installation,
            )?,
            name,
            "the anchor Cargo.lock does not identify an installation closure at the declared version and configured sources",
        )?
    } else {
        Closure::new()
    };
    let work = if work_package.has_lockfile_target {
        let lockfile = cache.work(work_tree, name, &git_path)?;
        required_closure(
            lockfile.closure(
                name,
                &work_package.manifest.version.to_string(),
                &work_tree.installation,
            )?,
            name,
            "the work-tree Cargo.lock does not identify an installation closure at the declared version and configured sources; refresh Cargo.lock",
        )?
    } else {
        Closure::new()
    };
    Ok(closure_changes(&anchor, &work))
}

fn required_closure(
    closure: Option<Closure>,
    package: &str,
    reason: &str,
) -> Result<Closure, AppError> {
    closure.ok_or_else(|| LockfileClosureUnavailableError::new(package, reason).into())
}

/// Reads lockfile bytes as text, naming `path` if they are not text at all.
fn decode_lockfile(bytes: Vec<u8>, path: &str) -> Result<String, AppError> {
    String::from_utf8(bytes).map_err(|error| MalformedLockfileError::caused_by(path, error).into())
}

fn log_untracked(notes: &impl NoteSink, name: &str, count: usize) {
    if count == 0 {
        return;
    }
    notes.note(|| {
        format!(
            "{}: {} match packaging rules and are advisory only; released content is defined as \
         the git-tracked files under the package, so untracked paths are never counted as changes \
         even where Cargo would pack them (an untracked nested manifest therefore does not draw a \
         package boundary either; the optional packaging probe warns about that divergence)",
            quote_path(name),
            plural(count, "untracked path")
        )
    });
}

// Early-exit is equivalent to walking the rest of first-parent history.
#[cfg_attr(test, mutants::skip)]
fn can_stop_timeline(timeline: &[TimelineEntry]) -> bool {
    let [.., last] = timeline else {
        return false;
    };
    // The anchor walk starts at the newest commit that released the package: the
    // history endpoint itself when it releases the package, the reintroduction point otherwise.
    // Until an older commit declares a different version the anchor is still
    // undetermined, and a package no commit has released yet needs the whole
    // history to tell creation from truncation.
    let Some(reference) = timeline
        .iter()
        .find_map(|entry| entry.presence.released_version())
    else {
        return false;
    };
    // A commit at which the package was withdrawn is invisible to the walk, so
    // it can never be the change that ends it. An absent package is visible: its
    // reappearance is itself a version change.
    !last.presence.is_unpublished() && last.presence.released_version() != Some(reference)
}

/// Reads a tracked work-tree path the way Cargo would pack it.
///
/// A symbolic link stops the run rather than being compared. Cargo dereferences
/// a link when it builds a package archive, while Git stores the link as a blob holding
/// the target path, so neither reading the target text nor following the link
/// yields a comparison that is right at both ends. Ref: packages/cargo-release-plan/docs/design.md,
/// "Released content".
// Native observations only; read_optional_bytes_with covers missing/error/link/byte distinctions.
#[cfg_attr(test, mutants::skip)]
pub fn read_optional_bytes(
    path: &Path,
    name: &str,
    rel: &str,
) -> Result<Option<Vec<u8>>, AppError> {
    read_optional_bytes_with(
        path,
        name,
        rel,
        fs::symlink_metadata(path).map(|metadata| metadata.file_type().is_symlink()),
        || fs::read(path),
    )
}

/// Interprets the filesystem observations without changing their order.
///
/// Injecting acquisition makes disappearance between metadata and reading
/// deterministic in tests, without racing another thread or using delays.
fn read_optional_bytes_with(
    path: &Path,
    name: &str,
    rel: &str,
    symlink: io::Result<bool>,
    read: impl FnOnce() -> io::Result<Vec<u8>>,
) -> Result<Option<Vec<u8>>, AppError> {
    match symlink {
        Ok(true) => {
            return Err(SymlinkReleasedError::new(name, rel).into());
        }
        Ok(false) => {}
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => return Err(ReadFileError::caused_by(path, error).into()),
    }
    match read() {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(error) => Err(ReadFileError::caused_by(path, error).into()),
    }
}

fn is_not_found(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound
}

/// Git-root-relative path of the workspace root manifest.
///
/// The prefix comes from Git rather than from subtracting Cargo's
/// `workspace_root` from the repository root, because the two tools can spell
/// the same directory differently and a failed subtraction would silently pick
/// the repository-root manifest for a nested workspace. Ref:
/// packages/cargo-release-plan/docs/implementation.md, "Classification".
#[must_use]
pub fn root_manifest_rel(git: &GitRepo) -> String {
    let prefix = git.prefix();
    if prefix.is_empty() {
        "Cargo.toml".to_string()
    } else {
        format!("{prefix}/Cargo.toml")
    }
}

/// Rebases a git-root-relative directory onto the workspace root.
///
/// `workspace_prefix` is the git-root-relative workspace directory with a trailing
/// separator, or empty when the workspace root is the git root. Leading parent
/// components preserve members that use `[package] workspace` from beside a
/// nested workspace root.
fn workspace_relative_dir(dir: &str, workspace_prefix: &str, case: PathCase) -> String {
    let workspace: Vec<&str> = workspace_prefix
        .trim_matches('/')
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    let directory: Vec<&str> = dir
        .trim_matches('/')
        .split('/')
        .filter(|component| !component.is_empty())
        .collect();
    let common = workspace
        .iter()
        .zip(&directory)
        .take_while(|(left, right)| case.same_path(left, right))
        .count();
    let mut relative = vec![".."; workspace.len().saturating_sub(common)];
    relative.extend(directory.iter().skip(common).copied());
    relative.join("/")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod diagnostic_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod discovery_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod installation_tests;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    #[test]
    fn absent_binary_closure_is_not_an_empty_successful_assessment() {
        let error = required_closure(None, "binary", "missing endpoint").unwrap_err();
        assert!(
            error
                .find_source::<LockfileClosureUnavailableError>()
                .is_some()
        );
        assert!(
            required_closure(Some(Closure::new()), "binary", "unused")
                .unwrap()
                .is_empty()
        );
    }

    use crp_workspace::git::testing::unopened;
    use crp_workspace::inherited::InheritedKeys;
    use crp_workspace::manifest::{InstallationDependencies, TargetDiscovery};

    use super::*;

    #[test]
    fn snapshot_case_controls_path_identity() {
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let mut cache = SnapshotCache::default();
            cache.bind(
                &unopened(Path::new("repository")),
                Path::new("workspace"),
                case,
                BTreeMap::new(),
            );
            assert_eq!(
                cache.case().same_path("a", "A"),
                case == PathCase::Insensitive
            );
        }
    }

    fn empty_snapshot() -> CommitSnapshot {
        CommitSnapshot {
            tree: Rc::default(),
            packages: BTreeMap::new(),
            unpublished: BTreeSet::new(),
            root_doc: DocumentMut::new(),
            installation: InstallationGraph::default(),
        }
    }

    #[test]
    fn snapshots_reuse_only_successful_observations_of_the_requested_commit() {
        let git = unopened(Path::new("repository"));
        let mut cache = SnapshotCache::default();
        cache.bind(
            &git,
            Path::new("workspace"),
            PathCase::Sensitive,
            BTreeMap::new(),
        );
        let first = cache
            .snapshot_with("first", |_| Ok(empty_snapshot()))
            .unwrap();
        cache.lockfiles.insert(
            "first".to_string(),
            Lockfile::parse("version = 4", "lock").unwrap(),
        );
        cache.bind(
            &git,
            Path::new("workspace"),
            PathCase::Sensitive,
            BTreeMap::new(),
        );
        let reused = cache.snapshot_with("first", |_| panic!("cached")).unwrap();
        assert!(Rc::ptr_eq(&first, &reused));
        assert!(cache.lockfiles.contains_key("first"));
        let error = cache
            .snapshot_with("second", |_| Err(io::Error::other("snapshot").into()))
            .unwrap_err();
        assert!(error.find_source::<io::Error>().is_some());
        assert_eq!(cache.inner.len(), 1);
        let second = cache
            .snapshot_with("second", |_| Ok(empty_snapshot()))
            .unwrap();
        assert!(!Rc::ptr_eq(&first, &second));
        assert_eq!(cache.inner.len(), 2);
    }

    #[test]
    fn git_interpretation_changes_invalidate_every_retained_observation() {
        let mut context = serde_json::to_value(GitObjectContext::default()).unwrap();
        *context.get_mut("format").unwrap() = "sha1".into();
        let context: GitObjectContext = serde_json::from_value(context).unwrap();
        let mut cache = SnapshotCache::default();
        cache
            .inner
            .insert("commit".into(), Rc::new(empty_snapshot()));
        cache.lockfiles.insert(
            "commit".into(),
            Lockfile::parse("version = 4", "lock").unwrap(),
        );
        cache.headers.parent_with("commit", || Ok(false)).unwrap();
        cache.bind_objects(context.clone());
        assert_eq!(cache.objects, context);
        assert!(cache.inner.is_empty());
        assert!(cache.lockfiles.is_empty());
        assert!(cache.headers.parent_with("commit", || Ok(true)).unwrap());

        cache
            .inner
            .insert("commit".into(), Rc::new(empty_snapshot()));
        cache.bind_objects(context);
        assert_eq!(cache.inner.len(), 1);
        assert!(cache.headers.parent_with("commit", || panic!()).unwrap());
    }

    #[test]
    fn every_snapshot_interpretation_input_invalidates_reuse() {
        let git = unopened(Path::new("repository"));
        let workspace = Path::new("workspace");
        let registries = BTreeMap::from([("custom".to_string(), "registry-index".to_string())]);
        let mut different_root = git.clone();
        different_root.root = PathBuf::from("another-repository");
        let mut different_prefix = git.clone();
        different_prefix.prefix = "nested".to_string();
        for (git, workspace, case, registries) in [
            (
                different_root,
                workspace,
                PathCase::Sensitive,
                registries.clone(),
            ),
            (
                different_prefix,
                workspace,
                PathCase::Sensitive,
                registries.clone(),
            ),
            (
                git.clone(),
                Path::new("another-workspace"),
                PathCase::Sensitive,
                registries.clone(),
            ),
            (git.clone(), workspace, PathCase::Insensitive, registries),
            (git, workspace, PathCase::Sensitive, BTreeMap::new()),
        ] {
            let mut cache = SnapshotCache::default();
            cache.bind(
                &unopened(Path::new("repository")),
                Path::new("workspace"),
                PathCase::Sensitive,
                BTreeMap::from([("custom".to_string(), "registry-index".to_string())]),
            );
            let before = cache
                .snapshot_with("commit", |_| Ok(empty_snapshot()))
                .unwrap();
            cache.lockfiles.insert(
                "commit".to_string(),
                Lockfile::parse("version = 4", "lock").unwrap(),
            );
            assert!(!cache.headers.parent_with("commit", || Ok(false)).unwrap());
            cache.bind(&git, workspace, case, registries);
            assert!(cache.lockfiles.is_empty());
            assert!(cache.headers.parent_with("commit", || Ok(true)).unwrap());
            let after = cache
                .snapshot_with("commit", |_| Ok(empty_snapshot()))
                .unwrap();
            assert!(!Rc::ptr_eq(&before, &after));
        }
    }

    #[test]
    fn historical_tree_selection_resolves_external_resource_aliases() {
        let tree = [
            "100644 blob manifest\tpackage/Cargo.toml",
            "100644 blob source\tPACKAGE/src/lib.rs",
            "100755 blob readme\tDocs/readme.md",
            "100644 blob unrelated\tother/src/lib.rs",
        ]
        .map(|record| TreeEntry::parse(record).unwrap());
        let rules = PackagingRules::default();
        let resources = BTreeMap::from([("README.md".to_string(), "docs/README.md".to_string())]);
        for case in [PathCase::Sensitive, PathCase::Insensitive] {
            let side = PackageSide {
                dir: "package",
                rules: &rules,
                resources: &resources,
                auto_readme: false,
                case,
            };
            let selected = HistoricalTree::new(tree.to_vec());
            let released = released_at_commit(&selected, &side);
            assert_eq!(
                released.get("Cargo.toml").map(String::as_str),
                Some("package/Cargo.toml")
            );
            assert!(!released.values().any(|path| path.starts_with("other/")));
            match case {
                PathCase::Sensitive => {
                    assert!(!released.contains_key("README.md"));
                    assert!(!released.contains_key("src/lib.rs"));
                }
                PathCase::Insensitive => {
                    assert_eq!(released.get("src/lib.rs").unwrap(), "PACKAGE/src/lib.rs");
                    assert_eq!(released.get("README.md").unwrap(), "Docs/readme.md");
                    let entry = selected.entry("Docs/readme.md").unwrap();
                    assert_eq!(entry.id, "readme");
                    assert!(entry.is_executable());
                }
            }
        }
    }

    #[test]
    fn released_paths_follow_case_aware_package_boundaries() {
        let paths = [
            "packages/demo/Cargo.toml",
            "packages/demo/src/lib.rs",
            "packages/demo/nested/cargo.toml",
            "packages/demo/nested/src/lib.rs",
            "packages/demolition/src/lib.rs",
        ]
        .map(str::to_string);
        let rules = PackagingRules::default();
        let resources = BTreeMap::new();
        let side = PackageSide {
            dir: "PACKAGES/DEMO",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Insensitive,
        };
        assert_eq!(
            released_from_paths(&paths, &paths, &side)
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", "src/lib.rs"])
        );
        assert!(
            released_from_paths(
                &paths,
                &paths,
                &PackageSide {
                    case: PathCase::Sensitive,
                    ..side
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn status_requires_consistent_anchor_version_and_change_evidence() {
        let anchor = Version::new(1, 0, 0);
        for (declared, previous, changed, expected) in [
            (
                Version::new(1, 0, 0),
                None,
                false,
                Some(PackageStatus::PendingRelease),
            ),
            (Version::new(1, 0, 0), None, true, None),
            (
                Version::new(1, 0, 0),
                Some(&anchor),
                false,
                Some(PackageStatus::Unchanged),
            ),
            (
                Version::new(1, 0, 0),
                Some(&anchor),
                true,
                Some(PackageStatus::NeedsIncrement),
            ),
            (
                Version::new(1, 0, 1),
                Some(&anchor),
                false,
                Some(PackageStatus::PendingRelease),
            ),
            (
                Version::new(1, 0, 1),
                Some(&anchor),
                true,
                Some(PackageStatus::PendingRelease),
            ),
            (Version::new(0, 9, 0), Some(&anchor), false, None),
            (Version::new(0, 9, 0), Some(&anchor), true, None),
        ] {
            assert_eq!(
                PackageStatus::from_evidence(&declared, previous, changed),
                expected
            );
        }
    }

    #[test]
    fn anchored_classification_rejects_regression_with_or_without_content_changes() {
        // The digit boundary distinguishes parsed version ordering from lexical ordering.
        let anchor = Anchor {
            commit: "released".to_owned(),
            version: Version::new(1, 10, 0),
        };
        for changed in [
            Vec::new(),
            vec![ChangedItem::Inherited {
                field: "package.rust-version".to_owned(),
            }],
        ] {
            let error = Verdict::anchored(
                "library",
                &Version::new(1, 9, 0),
                anchor.clone(),
                changed,
                String::new(),
            )
            .unwrap_err();
            assert_eq!(
                error
                    .find_source::<VersionRegressionError>()
                    .unwrap()
                    .package(),
                "library"
            );
        }
    }

    #[test]
    fn anchored_classification_preserves_equal_and_increased_release_evidence() {
        let anchor = Anchor {
            commit: "released".to_owned(),
            version: Version::new(1, 9, 0),
        };
        for declared in [anchor.version.clone(), Version::new(1, 10, 0)] {
            for has_changes in [false, true] {
                let (changed, patch) = if has_changes {
                    (
                        vec![ChangedItem::Package {
                            path: "src/lib.rs".to_owned(),
                            change: "modified".to_owned(),
                        }],
                        "-old\n+new\n".to_owned(),
                    )
                } else {
                    (Vec::new(), String::new())
                };
                let verdict = Verdict::anchored(
                    "library",
                    &declared,
                    anchor.clone(),
                    changed.clone(),
                    patch.clone(),
                )
                .unwrap();
                let class = PackageClass::with_verdict(
                    "library",
                    declared.clone(),
                    verdict,
                    PathBuf::from("library/Cargo.toml"),
                );
                let expected = if declared == anchor.version {
                    if has_changes {
                        PackageStatus::NeedsIncrement
                    } else {
                        PackageStatus::Unchanged
                    }
                } else {
                    PackageStatus::PendingRelease
                };
                assert_eq!(class.status(), expected);
                let actual_anchor = class.anchor().unwrap();
                assert_eq!(actual_anchor.version, anchor.version);
                assert_eq!(actual_anchor.commit, anchor.commit);
                assert_eq!(class.changed(), changed);
                assert_eq!(class.patch(), patch);
            }
        }
    }

    #[test]
    fn workspace_relative_dir_rebases_onto_the_workspace_root() {
        // Workspace root is the git root: paths pass through unchanged.
        assert_eq!(
            workspace_relative_dir("packages/a", "", PathCase::Sensitive),
            "packages/a"
        );
        assert_eq!(workspace_relative_dir("", "", PathCase::Sensitive), "");

        // Workspace root is nested: the prefix is stripped so member globs match.
        assert_eq!(
            workspace_relative_dir("rust/packages/a", "rust/", PathCase::Sensitive),
            "packages/a"
        );
        assert_eq!(
            workspace_relative_dir("rust", "rust/", PathCase::Sensitive),
            ""
        );

        // Explicit members beside a nested root retain their parent traversal.
        assert_eq!(
            workspace_relative_dir("dotnet/packages/a", "rust/", PathCase::Sensitive),
            "../dotnet/packages/a"
        );
        assert_eq!(
            workspace_relative_dir("", "rust/", PathCase::Sensitive),
            ".."
        );
        assert_eq!(
            workspace_relative_dir("rust/packages/A", "Rust/", PathCase::Insensitive),
            "packages/A"
        );
        assert_eq!(
            workspace_relative_dir("rust/packages/A", "Rust/", PathCase::Sensitive),
            "../rust/packages/A"
        );
    }

    #[test]
    fn only_a_package_absent_from_the_snapshot_is_new_on_it() {
        let mut snapshot = CommitSnapshot {
            tree: Rc::default(),
            packages: BTreeMap::new(),
            unpublished: BTreeSet::new(),
            root_doc: DocumentMut::new(),
            installation: InstallationGraph::default(),
        };
        snapshot.packages.insert(
            "released".to_string(),
            HistoricalPackage {
                directory: "packages/released".to_string(),
                version: Version::new(0, 1, 0),
                packaging: PackagingRules::default(),
                resources: BTreeMap::new(),
                auto_readme: false,
                has_lockfile_target: false,
            },
        );
        snapshot.unpublished.insert("withdrawn".to_string());

        assert!(!is_new_at_snapshot(&snapshot, "released"));
        // Withdrawn in this snapshot is still present, so the group binds it.
        assert!(!is_new_at_snapshot(&snapshot, "withdrawn"));
        assert!(is_new_at_snapshot(&snapshot, "added"));
    }

    #[test]
    fn group_exemption_requires_absence_from_actual_history_and_the_anticipated_parent() {
        fn snapshot(presence: &str) -> CommitSnapshot {
            let mut snapshot = CommitSnapshot {
                tree: Rc::default(),
                packages: BTreeMap::new(),
                unpublished: BTreeSet::new(),
                root_doc: DocumentMut::new(),
                installation: InstallationGraph::default(),
            };
            match presence {
                "published" => {
                    snapshot.packages.insert(
                        "member".into(),
                        HistoricalPackage {
                            directory: "member".into(),
                            version: Version::new(1, 0, 0),
                            packaging: PackagingRules::default(),
                            resources: BTreeMap::new(),
                            auto_readme: false,
                            has_lockfile_target: false,
                        },
                    );
                }
                "unpublished" => {
                    snapshot.unpublished.insert("member".into());
                }
                "absent" => {}
                _ => panic!("unknown fixture presence"),
            }
            snapshot
        }

        for history in ["absent", "published", "unpublished"] {
            let history_snapshot = snapshot(history);
            assert_eq!(
                is_new_in_assessment(&history_snapshot, None, "member"),
                history == "absent"
            );
            for target in ["absent", "published", "unpublished"] {
                assert_eq!(
                    is_new_in_assessment(&history_snapshot, Some(&snapshot(target)), "member"),
                    (history, target) == ("absent", "absent")
                );
            }
        }
    }

    #[test]
    fn every_changed_item_source_serializes_its_payload() {
        let items = [
            ChangedItem::Package {
                path: "src/lib.rs".to_string(),
                change: "modified".to_string(),
            },
            ChangedItem::Inherited {
                field: "workspace.package.license".to_string(),
            },
            ChangedItem::Lockfile {
                dependency: "serde".to_string(),
                change: "modified".to_string(),
            },
        ];
        let values: Vec<serde_json::Value> = items
            .iter()
            .map(|item| serde_json::to_value(item).unwrap())
            .collect();

        let package = values.first().unwrap();
        let inherited = values.get(1).unwrap();
        let lockfile = values.get(2).unwrap();
        assert_eq!(package.get("source").unwrap(), "package");
        assert_eq!(package.get("path").unwrap(), "src/lib.rs");
        assert_eq!(inherited.get("source").unwrap(), "inherited");
        assert_eq!(inherited.get("field").unwrap(), "workspace.package.license");
        assert_eq!(lockfile.get("source").unwrap(), "lockfile");
        assert_eq!(lockfile.get("dependency").unwrap(), "serde");
    }

    #[test]
    fn is_not_found_matches_not_found_kind() {
        assert!(is_not_found(&io::Error::new(
            io::ErrorKind::NotFound,
            "missing",
        )));
        assert!(!is_not_found(&io::Error::new(
            io::ErrorKind::PermissionDenied,
            "denied",
        )));
    }

    #[test]
    fn join_relative_resolves_against_the_member_directory() {
        assert_eq!(
            join_relative("packages/a", "../b"),
            Some("packages/b".to_string())
        );
        assert_eq!(
            join_relative("packages/a", "./nested/"),
            Some("packages/a/nested".to_string())
        );
        assert_eq!(join_relative("", "vendored"), Some("vendored".to_string()));
        // Cargo resolves a manifest-declared path with the host's own rules, so
        // a backslash separates components only where the platform says it does
        // and is an ordinary file name character everywhere else.
        let expected = if MAIN_SEPARATOR == '\\' {
            "packages/b".to_string()
        } else {
            r"packages/a/..\b".to_string()
        };
        assert_eq!(join_relative("packages/a", r"..\b"), Some(expected));
        // Climbing above the workspace root leaves the workspace entirely.
        assert_eq!(join_relative("packages", "../../outside"), None);
    }

    #[test]
    fn nested_package_dirs_selects_strict_descendants() {
        let paths = vec![
            "Cargo.toml".to_string(),
            "packages/a/Cargo.toml".to_string(),
            "packages/a/src/lib.rs".to_string(),
            "packages/a/inner/Cargo.toml".to_string(),
            "packages/ab/Cargo.toml".to_string(),
        ];
        assert_eq!(
            nested_package_dirs(&paths, "packages/a", PathCase::Sensitive),
            vec!["packages/a/inner".to_string()]
        );
        assert_eq!(
            nested_package_dirs(&paths, "packages/a/inner", PathCase::Sensitive),
            Vec::<String>::new()
        );
        // A root package contains every other manifest.
        assert_eq!(
            nested_package_dirs(&paths, "", PathCase::Sensitive),
            vec![
                "packages/a".to_string(),
                "packages/a/inner".to_string(),
                "packages/ab".to_string(),
            ]
        );
    }

    #[test]
    fn nested_package_dirs_ignores_files_that_merely_end_in_the_manifest_name() {
        let paths = vec![
            "packages/a/Cargo.toml".to_string(),
            "packages/a/inner/NotCargo.toml".to_string(),
            "packages/a/inner/Cargo.toml.bak".to_string(),
        ];
        assert_eq!(
            nested_package_dirs(&paths, "packages/a", PathCase::Sensitive),
            Vec::<String>::new()
        );
    }

    #[test]
    fn released_from_paths_stops_at_a_nested_manifest() {
        let rules = PackagingRules::default();
        let resources = BTreeMap::new();
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Sensitive,
        };
        // `fixture` is a package of its own, so Cargo packs none of its files with
        // `packages/a` even though the workspace never lists it as a member.
        let paths = vec![
            "packages/a/Cargo.toml".to_string(),
            "packages/a/src/lib.rs".to_string(),
            "packages/a/fixture/Cargo.toml".to_string(),
            "packages/a/fixture/src/lib.rs".to_string(),
        ];
        let released = released_from_paths(&paths, &paths, &side);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", "src/lib.rs"])
        );
        assert_eq!(released.get("Cargo.toml").unwrap(), "packages/a/Cargo.toml");
    }

    /// A backslash in a reported path is not a directory boundary.
    ///
    /// Git's `-z` listings separate directories with `/` on every platform, so a `\` in a reported
    /// path belongs to a file's name. Rewriting it would file the content under a directory that
    /// does not exist and, worse, could make two distinct files collide on one package-relative
    /// key.
    #[test]
    fn a_backslash_in_a_reported_path_is_not_a_directory_boundary() {
        let rules = PackagingRules::default();
        let resources = BTreeMap::new();
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Sensitive,
        };
        let paths = vec![
            "packages/a/Cargo.toml".to_string(),
            r"packages/a/src/odd\name.rs".to_string(),
        ];
        let released = released_from_paths(&paths, &paths, &side);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", r"src/odd\name.rs"])
        );
        assert_eq!(
            released.get(r"src/odd\name.rs").unwrap(),
            r"packages/a/src/odd\name.rs"
        );
    }

    /// A detected readme outranks the packaging rules.
    ///
    /// Cargo packs the README it detects itself even when `include` omits it, and prefers the first
    /// of its default names that the end being examined holds.
    #[test]
    fn a_detected_readme_outranks_the_packaging_rules() {
        let rules = PackagingRules::new(Some(&["/src/".to_string()]), None).unwrap();
        let resources = BTreeMap::new();
        let paths = vec![
            "packages/a/src/lib.rs".to_string(),
            "packages/a/README.md".to_string(),
            "packages/a/README.txt".to_string(),
        ];
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: true,
            case: PathCase::Sensitive,
        };

        let released = released_from_paths(&paths, &paths, &side);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/lib.rs", "README.md"])
        );

        // A package that names its README, or disables the key, gets no detection.
        let declared = PackageSide {
            auto_readme: false,
            ..side
        };
        assert_eq!(
            released_from_paths(&paths, &paths, &declared)
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/lib.rs"])
        );
    }

    /// A detected readme follows the probed case rules.
    ///
    /// Cargo probes the filesystem for its default README names, so on a case-insensitive volume a
    /// tracked `readme.md` answers the `README.md` candidate and its content is released. Matching
    /// the spelling exactly there would report such a package as having released nothing.
    #[test]
    fn a_detected_readme_follows_the_probed_case_rules() {
        let rules = PackagingRules::new(Some(&["/src/".to_string()]), None).unwrap();
        let resources = BTreeMap::new();
        let paths = vec![
            "packages/a/src/lib.rs".to_string(),
            "packages/a/readme.md".to_string(),
        ];
        let strict = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: true,
            case: PathCase::Sensitive,
        };
        assert_eq!(
            released_from_paths(&paths, &paths, &strict)
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/lib.rs"])
        );

        let relaxed = PackageSide {
            case: PathCase::Insensitive,
            ..strict
        };
        let released = released_from_paths(&paths, &paths, &relaxed);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/lib.rs", "readme.md"])
        );
        // Keying by the tracked spelling keeps a re-spelling of the file
        // visible as the released-content change it is.
        assert_eq!(released.get("readme.md").unwrap(), "packages/a/readme.md");
    }

    /// A declared resource follows the probed case rules while retaining Git's spelling.
    ///
    /// Cargo opens the manifest's spelling through the filesystem. Git path and
    /// blob lookups must use the recorded spelling after deciding that both name
    /// the same resource.
    #[test]
    fn a_declared_resource_follows_the_probed_case_rules() {
        let rules = PackagingRules::default();
        let resources =
            BTreeMap::from([("README.md".to_string(), "packages/a/README.md".to_string())]);
        let tracked = vec!["packages/a/readme.md".to_string()];
        let strict = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Sensitive,
        };
        assert!(tracked_resources(&strict, &tracked).is_empty());

        let relaxed = PackageSide {
            case: PathCase::Insensitive,
            ..strict
        };
        assert_eq!(
            tracked_resources(&relaxed, &tracked),
            BTreeMap::from([("README.md".to_string(), "packages/a/readme.md".to_string())])
        );
    }

    /// A deleted default README is no longer detected.
    ///
    /// Git still lists a tracked file the work tree has deleted, but Cargo packages what is on
    /// disk, so README detection must use the paths that remain present.
    #[test]
    fn a_deleted_readme_is_no_longer_detected() {
        let resources = BTreeMap::new();
        // `include` omits the README, so only detection can bring it in.
        let rules = PackagingRules::new(Some(&["/src/".to_string()]), None).unwrap();
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: true,
            case: PathCase::Sensitive,
        };
        let tracked = vec![
            "packages/a/src/lib.rs".to_string(),
            "packages/a/README.md".to_string(),
        ];
        assert!(
            released_from_paths(&tracked, &tracked, &side).contains_key("README.md"),
            "a README on disk is detected"
        );
        let present = vec!["packages/a/src/lib.rs".to_string()];
        assert!(
            !released_from_paths(&tracked, &present, &side).contains_key("README.md"),
            "a README the work tree deleted is not"
        );
    }

    /// A deleted nested manifest no longer stops packing its directory.
    #[test]
    fn a_deleted_nested_manifest_no_longer_limits_released_content() {
        let resources = BTreeMap::new();
        let rules = PackagingRules::default();
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
            case: PathCase::Sensitive,
        };
        let tracked = vec![
            "packages/a/Cargo.toml".to_string(),
            "packages/a/fixture/Cargo.toml".to_string(),
            "packages/a/fixture/src/lib.rs".to_string(),
        ];
        let present = vec!["packages/a/Cargo.toml".to_string()];
        // Without the nested manifest on disk the boundary is gone and the
        // files beneath it become the outer package's released content.
        assert_eq!(
            released_from_paths(&tracked, &present, &side)
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", "fixture/Cargo.toml", "fixture/src/lib.rs"])
        );
    }

    /// Resources resolve against the end that declared them.
    ///
    /// A resource outside the package directory is released content under the name it takes at the
    /// crate root; one already inside keeps its own path.
    #[test]
    fn resources_resolve_against_the_end_that_declared_them() {
        let manifest = manifest_with_resources(
            "packages/a",
            &["../../LICENSE", "docs/GUIDE.md"],
            &["README.md"],
        );

        let resolved = resolve_resources(&manifest, "packages/a", "");

        assert_eq!(
            resolved,
            BTreeMap::from([
                ("LICENSE".to_string(), "LICENSE".to_string()),
                ("README.md".to_string(), "README.md".to_string()),
                (
                    "docs/GUIDE.md".to_string(),
                    "packages/a/docs/GUIDE.md".to_string()
                ),
            ])
        );
    }

    /// Inherited resources resolve against the workspace prefix.
    ///
    /// A nested workspace declares inherited resources relative to its own root, not to the git
    /// root.
    #[test]
    fn inherited_resources_resolve_against_the_workspace_prefix() {
        let manifest = manifest_with_resources("inner/packages/a", &[], &["README.md"]);

        let resolved = resolve_resources(&manifest, "inner/packages/a", "inner/");

        assert_eq!(
            resolved,
            BTreeMap::from([("README.md".to_string(), "inner/README.md".to_string())])
        );
    }

    /// A resource outside the repository is dropped.
    ///
    /// A path climbing above the git root names no file in the repository, so it contributes no
    /// released content rather than failing classification.
    #[test]
    fn a_resource_outside_the_repository_is_dropped() {
        let manifest = manifest_with_resources("packages/a", &["../../../elsewhere/LICENSE"], &[]);

        assert!(resolve_resources(&manifest, "packages/a", "").is_empty());
    }

    fn manifest_with_resources(
        directory: &str,
        local: &[&str],
        inherited: &[&str],
    ) -> PackageManifest {
        PackageManifest {
            name: "a".to_string(),
            version: "0.1.0".parse().unwrap(),
            directory: directory.to_string(),
            packaging: PackagingRules::default(),
            inherited: InheritedKeys::default(),
            publish: true,
            path_dependencies: Vec::new(),
            inherited_path_dependencies: Vec::new(),
            installation_dependencies: InstallationDependencies::default(),
            resource_paths: local.iter().map(|path| (*path).to_string()).collect(),
            inherited_resource_paths: inherited.iter().map(|path| (*path).to_string()).collect(),
            auto_readme: false,
            targets: TargetDiscovery::default(),
        }
    }

    #[test]
    fn can_stop_timeline_waits_for_a_change_from_the_newest_carried_version() {
        fn entry(commit: &str, version: Option<&str>) -> TimelineEntry {
            TimelineEntry {
                commit: commit.to_string(),
                presence: version.map_or(Presence::Absent, |text| {
                    Presence::Published(text.parse().unwrap())
                }),
                has_parent: true,
            }
        }

        fn unpublished(commit: &str) -> TimelineEntry {
            TimelineEntry {
                commit: commit.to_string(),
                presence: Presence::Unpublished,
                has_parent: true,
            }
        }

        assert!(!can_stop_timeline(&[]));
        assert!(!can_stop_timeline(&[entry("c2", Some("0.1.0"))]));
        assert!(can_stop_timeline(&[
            entry("c2", Some("0.1.0")),
            entry("c1", Some("0.0.9")),
        ]));
        // Absent at the history endpoint and never carried: creation cannot be told from
        // truncation until the walk reaches a root.
        assert!(!can_stop_timeline(&[entry("c2", None), entry("c1", None)]));
        // Absent at the history endpoint but carried earlier: the reintroduction anchor is
        // still undetermined while the carried version keeps repeating.
        assert!(!can_stop_timeline(&[
            entry("c3", None),
            entry("c2", Some("0.3.0")),
        ]));
        assert!(!can_stop_timeline(&[
            entry("c3", None),
            entry("c2", Some("0.3.0")),
            entry("c1", Some("0.3.0")),
        ]));
        assert!(can_stop_timeline(&[
            entry("c3", None),
            entry("c2", Some("0.3.0")),
            entry("c1", Some("0.2.0")),
        ]));
        // A withdrawn commit releases nothing, so the walk must continue past it
        // to find the version change that actually ends the search.
        assert!(!can_stop_timeline(&[
            entry("c2", Some("0.3.0")),
            unpublished("c1"),
        ]));
        assert!(!can_stop_timeline(&[unpublished("c1")]));
    }

    #[test]
    fn is_inside_any_requires_a_directory_boundary() {
        let dirs = vec!["packages/a".to_string()];
        assert!(is_inside_any(
            "packages/a/src/lib.rs",
            &dirs,
            PathCase::Sensitive
        ));
        assert!(!is_inside_any("packages/a", &dirs, PathCase::Sensitive));
        assert!(!is_inside_any(
            "packages/ab/src/lib.rs",
            &dirs,
            PathCase::Sensitive
        ));
    }

    /// Serves pre-parsed manifests.
    ///
    /// Membership resolution can then be exercised without a repository.
    struct FakeManifests {
        manifests: BTreeMap<String, PackageManifest>,
        /// Directories whose manifest was actually read, in order of first read.
        read: Vec<String>,
    }

    impl FakeManifests {
        fn new(entries: &[(&str, &str)]) -> Self {
            let manifests = entries
                .iter()
                .map(|(dir, content)| {
                    let path = format!("{dir}/Cargo.toml");
                    let parsed =
                        parse_package_manifest(content, &path, &WorkspaceInherit::default())
                            .unwrap()
                            .unwrap();
                    ((*dir).to_string(), parsed)
                })
                .collect();
            Self {
                manifests,
                read: Vec::new(),
            }
        }
    }

    impl ManifestSource for FakeManifests {
        fn candidate_dirs(&self) -> Vec<String> {
            self.manifests.keys().cloned().collect()
        }

        fn manifest(&mut self, dir: &str) -> Result<Option<&PackageManifest>, AppError> {
            if !self.read.iter().any(|seen| seen == dir) {
                self.read.push(dir.to_string());
            }
            Ok(self.manifests.get(dir))
        }
    }

    #[test]
    fn resolve_members_follows_path_dependencies() {
        let root = Path::new("Cargo.toml");
        let members = parse_workspace_members(
            "[workspace]\nmembers = [\"a\"]\nexclude = [\"c\"]\n",
            root,
            PathCase::Sensitive,
        )
        .unwrap();
        // Traversal consumes parsed models. Seed the package identities from one small document;
        // dependency-table parsing and aggregation have their own manifest tests.
        let mut manifests =
            FakeManifests::new(&[("a", "[package]\nname = \"a\"\nversion = \"0.1.0\"\n")]);
        let package = manifests.manifests.get("a").unwrap().clone();
        for name in ["b", "c"] {
            let mut package = package.clone();
            package.name = name.to_string();
            package.directory = name.to_string();
            manifests.manifests.insert(name.to_string(), package);
        }
        manifests.manifests.get_mut("a").unwrap().path_dependencies =
            vec!["../b".to_string(), "../c".to_string()];
        let resolved = resolve_members(&mut manifests, &members).unwrap();
        // `b` is reachable only as a path dependency; `c` is excluded even though
        // a member depends on it.
        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    /// Resolve members does not read unreachable manifests.
    ///
    /// A manifest Cargo would never load for this workspace is never parsed, so an unrelated nested
    /// workspace cannot fail classification.
    #[test]
    fn resolve_members_does_not_read_unreachable_manifests() {
        let root = Path::new("Cargo.toml");
        let members = parse_workspace_members(
            "[workspace]\nmembers = [\"packages/a\"]\n",
            root,
            PathCase::Sensitive,
        )
        .unwrap();
        let mut manifests = FakeManifests::new(&[
            (
                "packages/a",
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\n",
            ),
            (
                "vendor/unrelated",
                "[package]\nname = \"unrelated\"\nversion = \"0.1.0\"\n",
            ),
        ]);

        let resolved = resolve_members(&mut manifests, &members).unwrap();

        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["packages/a".to_string()]
        );
        assert_eq!(manifests.read, vec!["packages/a".to_string()]);
    }

    /// Resolve members ignores a path dependency outside the repository.
    ///
    /// A path dependency that climbs out of the repository is not a workspace member, and following
    /// it would name a directory outside the tree.
    #[test]
    fn resolve_members_ignores_a_path_dependency_outside_the_repository() {
        let root = Path::new("Cargo.toml");
        let members = parse_workspace_members(
            "[workspace]\nmembers = [\"packages/a\"]\n",
            root,
            PathCase::Sensitive,
        )
        .unwrap();
        let mut manifests = FakeManifests::new(&[(
            "packages/a",
            "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nout = { path = \"../../../outside\" }\n",
        )]);

        let resolved = resolve_members(&mut manifests, &members).unwrap();

        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["packages/a".to_string()]
        );
    }
}
