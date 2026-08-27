// Classification of publishable packages against their anchors.

use std::any::type_name;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::{fs, io, str};

use ohno::AppError;
use semver::Version;
use serde::ser::SerializeStruct;
use serde::{Serialize, Serializer};
use toml_edit::DocumentMut;

use crate::anchor::{Anchor, TimelineEntry, reintroduction_anchor, resolve_anchor};
use crate::diff::file_diff;
use crate::git::{GitRepo, join_git_rel};
use crate::groups::GroupVerdict;
use crate::inherited::{InheritedChange, inherited_changes};
use crate::manifest::{
    DEFAULT_README_FILES, PackageManifest, PathCase, WorkspaceInherit, WorkspaceMembers,
    is_workspace_excluded, is_workspace_member, parse_document, parse_package_manifest,
    parse_workspace_members,
};
use crate::metadata::{ReportedDep, WorkPackage, WorkTree, dependents_of, load_work_tree};
use crate::packaging::{PackagingRules, relativize};
use crate::text::plural;
use crate::verbose::Verbose;
use crate::{ReadFileError, VersionRegressionError, short_commit};

/// Workspace classification: every publishable package plus its group verdicts.
///
/// This is the result the `report` and `check` commands render.
#[derive(Debug)]
pub(crate) struct Classification {
    pub(crate) head: String,
    pub(crate) packages: Vec<PackageClass>,
    pub(crate) groups: BTreeMap<String, GroupVerdict>,
    pub(crate) work_tree: WorkTree,
    pub(crate) git: GitRepo,
}

/// Per-package classification: its status and the evidence behind it.
#[derive(Clone, Debug)]
pub(crate) struct PackageClass {
    pub(crate) name: String,
    pub(crate) declared_version: Version,
    pub(crate) group: Option<String>,
    pub(crate) status: PackageStatus,
    pub(crate) anchor: Option<Anchor>,
    pub(crate) changed: Vec<ChangedItem>,
    pub(crate) stat: DiffStat,
    pub(crate) patch: String,
    pub(crate) untracked: Vec<String>,
    pub(crate) dependencies: Vec<ReportedDep>,
    pub(crate) dependents: Vec<String>,
    pub(crate) manifest_path: PathBuf,
}

/// Classification status of one publishable package.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PackageStatus {
    Releasing,
    UnreleasedChanges,
    Released,
}

/// A released-content or inherited-value change.
///
/// Serialized as the report.json object with `path`/`change` or `field`, plus
/// `source`, so callers keep a stable JSON shape without optional nulls.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ChangedItem {
    Package { path: String, change: String },
    Inherited { field: String },
}

impl Serialize for ChangedItem {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // The struct name reaches self-describing formats, so derive it from the
        // type rather than repeating its spelling in a literal.
        let name = type_name::<Self>()
            .rsplit("::")
            .next()
            .unwrap_or("ChangedItem");
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
        }
    }
}

/// Insertion/deletion counts for one package in `report.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct DiffStat {
    pub(crate) files: usize,
    pub(crate) insertions: usize,
    pub(crate) deletions: usize,
}

/// Anchor identity as serialized in `report.json`.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct AnchorJson {
    pub(crate) commit: String,
    pub(crate) version: String,
}

pub(crate) fn classify(
    manifest_path: &Path,
    base: &str,
    verbose: Verbose,
) -> Result<Classification, AppError> {
    let mut work_tree = load_work_tree(manifest_path)?;
    let git = GitRepo::discover(&work_tree.workspace_root)?;
    for package in &mut work_tree.packages {
        package.manifest.directory = join_git_rel(git.prefix(), &package.manifest.directory);
        package.resources =
            resolve_resources(&package.manifest, &package.manifest.directory, git.prefix());
    }
    let head = git.head()?;
    let base_sha = git.rev_parse(base)?;
    verbose.note(format!(
        "classifying {} against base {base} ({base_sha}); \
         anchors are the last parsed version change on that revision's first-parent line, \
         not on the work tree's branch",
        plural(work_tree.packages.len(), "publishable package")
    ));

    let mut cache = SnapshotCache::new(&work_tree.workspace_root);
    let base_snapshot = cache.snapshot(&git, &base_sha)?;
    let work_root_path = work_tree.workspace_root.join("Cargo.toml");
    let work_root_doc = parse_document(
        &work_root_path,
        &fs::read_to_string(&work_root_path)
            .map_err(|error| ReadFileError::caused_by(&work_root_path, error))?,
    )?;

    let commits = git.first_parent_manifest_commits(&base_sha)?;
    let mut classes = Vec::new();
    let mut exempt = HashSet::new();
    let mut versions = BTreeMap::new();

    for package in &work_tree.packages {
        versions.insert(
            package.manifest.name.clone(),
            package.manifest.version.clone(),
        );
        let class = classify_one(
            package,
            &work_tree,
            &git,
            &base_sha,
            &commits,
            &base_snapshot,
            &work_root_doc,
            &mut cache,
            verbose,
        )?;
        // The exemption is base membership, not anchor presence: a reintroduced
        // package is absent from the base yet still resolves an older anchor, and
        // the documented rule exempts every member that does not exist on the base
        // revision from matching its group's declared version.
        if !base_snapshot.packages.contains_key(&package.manifest.name) {
            exempt.insert(package.manifest.name.clone());
        }
        classes.push(class);
    }

    let group_verdicts = work_tree.groups.verdicts(&versions, &exempt);
    for (name, verdict) in &group_verdicts {
        verbose.note(format!(
            "version group {name}: members [{}]; consistent={} (members that do not exist on \
             the base revision are exempt from matching declared versions)",
            verdict.members.join(", "),
            verdict.consistent
        ));
    }

    Ok(Classification {
        head,
        packages: classes,
        groups: group_verdicts,
        work_tree,
        git,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "classification needs the work-tree package, both trees, the first-parent walk, and the snapshot cache together"
)]
fn classify_one(
    package: &WorkPackage,
    work_tree: &WorkTree,
    git: &GitRepo,
    base_sha: &str,
    commits: &[String],
    base_snapshot: &CommitSnapshot,
    work_root_doc: &DocumentMut,
    cache: &mut SnapshotCache,
    verbose: Verbose,
) -> Result<PackageClass, AppError> {
    let name = &package.manifest.name;
    let group = work_tree.groups.group_of(name).map(ToOwned::to_owned);
    let dependents = dependents_of(&work_tree.packages, name);

    let timeline = build_timeline(git, name, commits, cache)?;
    let anchor = if base_snapshot.packages.contains_key(name) {
        resolve_anchor(name, &timeline)?
    } else {
        match reintroduction_anchor(name, &timeline)? {
            Some(anchor) => {
                verbose.note(format!(
                    "{name}: absent from base {base_sha} but carried earlier on its first-parent \
                     history, so this branch reintroduces a package rather than creating one and \
                     the last version change on that history ({} declaring {}) is the anchor",
                    short_commit(&anchor.commit),
                    anchor.version
                ));
                anchor
            }
            None => {
                verbose.note(format!(
                    "{name}: absent from base {base_sha} and from every sampled commit on its \
                     first-parent history, so creation on this branch counts as a version \
                     increase and the status is releasing"
                ));
                let side = work_tree_side(package);
                let resource_paths: Vec<&str> =
                    side.resources.values().map(String::as_str).collect();
                let tracked_resources = git.tracked_paths(&resource_paths)?;
                let untracked = untracked_released(git, &side, &tracked_resources)?;
                log_untracked(verbose, name, untracked.len());
                return Ok(PackageClass {
                    name: name.clone(),
                    declared_version: package.manifest.version.clone(),
                    group,
                    status: PackageStatus::Releasing,
                    anchor: None,
                    changed: Vec::new(),
                    stat: DiffStat {
                        files: 0,
                        insertions: 0,
                        deletions: 0,
                    },
                    patch: String::new(),
                    untracked,
                    dependencies: package.dependencies.clone(),
                    dependents,
                    manifest_path: package.manifest_path.clone(),
                });
            }
        }
    };
    let short_anchor = short_commit(&anchor.commit);
    verbose.note(format!(
        "{name}: anchor {short_anchor} declared {}; work tree declares {}; a status of releasing \
         requires the work-tree version to be greater than the anchor version (parsed, not textual)",
        anchor.version,
        package.manifest.version
    ));

    let anchor_snapshot = cache.snapshot(git, &anchor.commit)?;
    let anchor_pkg = anchor_snapshot
        .packages
        .get(name)
        .expect("the anchor commit is the newest commit at which the anchor version was observed, and both the timeline and this snapshot read that version from the same cache, so the package is present here");

    let (changed_files, mut patch, stat, untracked) = diff_package(
        git,
        &anchor.commit,
        &PackageSide {
            dir: &anchor_pkg.directory,
            rules: &anchor_pkg.packaging,
            resources: &anchor_pkg.resources,
            auto_readme: anchor_pkg.auto_readme,
        },
        &work_tree_side(package),
    )?;

    let mut changed = changed_files;
    let inherited: Vec<InheritedChange> = inherited_changes(
        &package.manifest.inherited,
        &anchor_snapshot.root_doc,
        work_root_doc,
    );
    for item in inherited {
        verbose.note(format!(
            "{name}: inherited {} changed between the anchor and the work tree, so the root \
             manifest is in scope for this package",
            item.field
        ));
        changed.push(ChangedItem::Inherited { field: item.field });
    }

    log_untracked(verbose, name, untracked.len());

    // A declared version below the anchor cannot describe a release: the anchor
    // version is already on the base line, so the work tree would re-publish an
    // existing version with different content. Ref: docs/design.md,
    // "Version monotonicity".
    if package.manifest.version < anchor.version {
        return Err(VersionRegressionError::new(
            name,
            package.manifest.version.clone(),
            anchor.version.clone(),
            &anchor.commit,
        )
        .into());
    }

    let version_increased = package.manifest.version > anchor.version;
    let status = if version_increased {
        PackageStatus::Releasing
    } else if changed.is_empty() {
        PackageStatus::Released
    } else {
        PackageStatus::UnreleasedChanges
    };
    verbose.note(format!(
        "{name}: status {status:?} because version_increased={version_increased} and \
         changed_items={}",
        changed.len()
    ));

    if status != PackageStatus::UnreleasedChanges {
        patch.clear();
    }

    Ok(PackageClass {
        name: name.clone(),
        declared_version: package.manifest.version.clone(),
        group,
        status,
        anchor: Some(anchor),
        changed,
        stat,
        patch,
        untracked,
        dependencies: package.dependencies.clone(),
        dependents,
        manifest_path: package.manifest_path.clone(),
    })
}

fn build_timeline(
    git: &GitRepo,
    name: &str,
    commits: &[String],
    cache: &mut SnapshotCache,
) -> Result<Vec<TimelineEntry>, AppError> {
    let mut timeline = Vec::with_capacity(commits.len());
    for (index, commit) in commits.iter().enumerate() {
        let snapshot = cache.snapshot(git, commit)?;
        let version = snapshot.packages.get(name).map(|pkg| pkg.version.clone());
        let is_last = index
            .checked_add(1)
            .is_some_and(|next| next == commits.len());
        let has_parent = if is_last {
            git.has_parent_or_is_shallow_boundary(commit)?
        } else {
            true
        };
        timeline.push(TimelineEntry {
            commit: commit.clone(),
            version,
            has_parent,
        });
        // Stop once we have observed a version different from the base (the
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
struct PackageSide<'a> {
    dir: &'a str,
    rules: &'a PackagingRules,
    /// Files Cargo packs because a manifest key names them, keyed by the path
    /// each takes inside the `.crate` and valued by its git-root-relative path.
    resources: &'a BTreeMap<String, String>,
    /// Whether Cargo picks this package's README by probing its directory.
    auto_readme: bool,
}

/// Resolves the files Cargo copies into the `.crate` because a manifest key
/// names them.
///
/// Cargo packs the file named by `readme` or `license-file` regardless of
/// `include` and `exclude`, and from outside `package_dir` if that is where it
/// lives. A package's released content is therefore neither confined to its own
/// directory nor fully described by its packaging rules: a workspace-level
/// README that several members inherit is released content for every one of
/// them, and so is a README the package's own `include` leaves out.
/// Ref: docs/design.md, "Released content".
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
        let key = match relativize(&full, package_dir) {
            Some(rel) => rel.to_string(),
            None => {
                let Some(name) = full.rsplit('/').next() else {
                    continue;
                };
                name.to_string()
            }
        };
        resolved.insert(key, full);
    }
    resolved
}

fn diff_package(
    git: &GitRepo,
    anchor_commit: &str,
    anchor: &PackageSide<'_>,
    work: &PackageSide<'_>,
) -> Result<(Vec<ChangedItem>, String, DiffStat, Vec<String>), AppError> {
    // Released content is defined from git-tracked files, and a manifest
    // resource may sit outside the package directory or outside its packaging
    // rules, where the directory listing cannot vouch for it. Asking Git
    // directly keeps an untracked README from being read off disk and reported
    // as a content change. Ref: docs/design.md, "Released content".
    let resource_paths: Vec<&str> = work.resources.values().map(String::as_str).collect();
    let tracked_resources = git.tracked_paths(&resource_paths)?;

    let anchor_files = released_at_commit(git, anchor_commit, anchor)?;
    let work_files = released_in_work_tree(git, work, &tracked_resources)?;

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
        let old = match anchor_files.get(rel) {
            Some(path) => git.show_file_bytes(anchor_commit, path)?,
            None => None,
        };
        let new = match work_files.get(rel) {
            Some(path) => read_optional_bytes(&git.root().join(path))?,
            None => None,
        };
        if old.as_deref() == new.as_deref() {
            continue;
        }
        let kind = match (old.is_some(), new.is_some()) {
            (false, true) => "added",
            (true, false) => "deleted",
            _ => "modified",
        };
        changed.push(ChangedItem::Package {
            path: rel.to_string(),
            change: kind.to_string(),
        });
        let file_diff = file_diff(rel, old.as_deref(), new.as_deref());
        insertions = insertions.saturating_add(file_diff.insertions);
        deletions = deletions.saturating_add(file_diff.deletions);
        patch.push_str(&file_diff.text);
    }

    let untracked = untracked_released(git, work, &tracked_resources)?;

    let stat = DiffStat {
        files: changed.len(),
        insertions,
        deletions,
    };
    Ok((changed, patch, stat, untracked))
}

fn released_at_commit(
    git: &GitRepo,
    commit: &str,
    side: &PackageSide<'_>,
) -> Result<HashMap<String, String>, AppError> {
    let mut released = released_from_paths(&git.ls_tree(commit, side.dir)?, side);
    // Reading a resource back from the commit yields nothing when the commit
    // did not track it, so the tree itself performs the tracked-only filter the
    // work tree needs `tracked_paths` for.
    add_resources(&mut released, side.resources.iter());
    Ok(released)
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

/// Lists the untracked paths beneath a package that its packaging rules would
/// release, package-relative.
///
/// These are advisory only: released content is defined from git-tracked files,
/// so an untracked path is never a change. Ref: docs/design.md, "Released
/// content".
fn untracked_released(
    git: &GitRepo,
    side: &PackageSide<'_>,
    tracked_resources: &HashSet<String>,
) -> Result<Vec<String>, AppError> {
    let listed: Vec<String> = git.ls_untracked(side.dir)?;
    // The same nested-package boundary the tracked listing observes applies
    // here, or a file under a nested crate would be advertised as content
    // Cargo would pack for the outer one. The manifest drawing that boundary
    // may itself still be untracked, so both listings feed the scan.
    let tracked: Vec<String> = git.ls_files(side.dir)?;
    let mut boundary_paths = listed.clone();
    boundary_paths.extend_from_slice(&tracked);
    let nested = nested_package_dirs(&boundary_paths, side.dir);

    let mut untracked: Vec<String> = listed
        .iter()
        .filter(|full| !is_inside_any(full, &nested))
        .filter_map(|full| {
            let rel = relativize(full, side.dir)?.to_string();
            side.rules.is_released(&rel).then_some(rel)
        })
        .collect();
    // The listing above is filtered by the packaging rules and stops at the
    // package directory, so a resource that those rules exclude or that is
    // declared from outside would go unmentioned even though Cargo would pack
    // it. It is advisory in exactly the same way, and it is named by the path
    // it takes inside the `.crate`.
    untracked.extend(side.resources.iter().filter_map(|(name, path)| {
        let present = git.root().join(path).symlink_metadata().is_ok();
        (present && !tracked_resources.contains(path)).then(|| name.clone())
    }));
    if side.auto_readme {
        // A README Cargo would detect is packed whatever the packaging rules
        // say, so an untracked one is worth mentioning — but only while no
        // tracked candidate outranks it, since that is the one Cargo picks.
        let tracked_set: HashSet<&str> = tracked.iter().map(String::as_str).collect();
        if detected_readme(side.dir, &tracked_set).is_none() {
            let listed_set: HashSet<&str> = listed.iter().map(String::as_str).collect();
            if let Some((name, _)) = detected_readme(side.dir, &listed_set) {
                untracked.push(name);
            }
        }
    }
    untracked.sort_unstable();
    untracked.dedup();
    Ok(untracked)
}

/// The package-relative paths of one work-tree package's released content.
///
/// The packaging verifier compares Cargo's own listing against this, so it has
/// to be the very selection classification compares — reconstructing the set
/// from `include` and `exclude` alone would drop a README Cargo detects for
/// itself and take in the files of a nested crate, reporting a mismatch on a
/// package whose rules are in fact right.
pub(crate) fn released_work_tree_paths(
    git: &GitRepo,
    package: &WorkPackage,
) -> Result<BTreeSet<String>, AppError> {
    let side = work_tree_side(package);
    let resource_paths: Vec<&str> = side.resources.values().map(String::as_str).collect();
    let tracked_resources = git.tracked_paths(&resource_paths)?;
    let released = released_in_work_tree(git, &side, &tracked_resources)?;
    Ok(released.into_keys().collect())
}

/// The work-tree end of `package`'s released-content comparison.
fn work_tree_side(package: &WorkPackage) -> PackageSide<'_> {
    PackageSide {
        dir: &package.manifest.directory,
        rules: &package.manifest.packaging,
        resources: &package.resources,
        auto_readme: package.manifest.auto_readme,
    }
}

fn released_in_work_tree(
    git: &GitRepo,
    side: &PackageSide<'_>,
    tracked_resources: &HashSet<String>,
) -> Result<HashMap<String, String>, AppError> {
    let mut released = released_from_paths(&git.ls_files(side.dir)?, side);
    add_resources(
        &mut released,
        side.resources
            .iter()
            .filter(|(_, path)| tracked_resources.contains(*path)),
    );
    Ok(released)
}

/// Selects one package's released content from the tracked paths beneath it.
///
/// Keys are package-relative, values are the git-root-relative paths the caller
/// reads the content back from.
fn released_from_paths(paths: &[String], side: &PackageSide<'_>) -> HashMap<String, String> {
    let nested = nested_package_dirs(paths, side.dir);
    let mut map = HashMap::new();
    for full in paths {
        if is_inside_any(full, &nested) {
            continue;
        }
        let Some(rel) = relativize(full, side.dir) else {
            continue;
        };
        if side.rules.is_released(rel) {
            map.insert(rel.to_string(), full.clone());
        }
    }
    if side.auto_readme {
        let present: HashSet<&str> = paths.iter().map(String::as_str).collect();
        if let Some((name, full)) = detected_readme(side.dir, &present) {
            map.entry(name).or_insert(full);
        }
    }
    map
}

/// The default README this end holds for `dir`, keyed by its name in the `.crate`.
///
/// Cargo probes the package directory for its default names in order and packs
/// the first that exists without consulting `include` or `exclude`, so a package
/// that names no README still releases the one beside it. Detection runs over
/// the same tracked listing the rest of the comparison uses, because released
/// content is defined from git-tracked files.
/// Ref: docs/design.md, "Released content".
fn detected_readme(dir: &str, present: &HashSet<&str>) -> Option<(String, String)> {
    DEFAULT_README_FILES.iter().find_map(|name| {
        let full = join_relative(dir, name)?;
        present
            .contains(full.as_str())
            .then(|| ((*name).to_string(), full))
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
/// otherwise non-member nested crate to the outer package and report changes it
/// will never release.
fn nested_package_dirs(paths: &[String], dir: &str) -> Vec<String> {
    let prefix = if dir.is_empty() {
        String::new()
    } else {
        format!("{dir}/")
    };
    paths
        .iter()
        .filter_map(|path| {
            let (parent, file) = path.rsplit_once('/')?;
            (file == "Cargo.toml").then_some(parent)
        })
        .filter(|parent| *parent != dir && parent.starts_with(&prefix))
        .map(ToOwned::to_owned)
        .collect()
}

fn is_inside_any(path: &str, dirs: &[String]) -> bool {
    dirs.iter().any(|dir| {
        path.strip_prefix(dir.as_str())
            .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Package facts reconstructed from a historical tree, keyed by package name.
#[derive(Clone, Debug)]
/// One publishable member as it existed at a historical commit.
struct HistoricalPackage {
    directory: String,
    version: Version,
    packaging: PackagingRules,
    /// Files Cargo packs because a manifest key names them, keyed by the path
    /// each takes inside the `.crate`.
    resources: BTreeMap<String, String>,
    /// Whether Cargo picks this package's README by probing its directory.
    auto_readme: bool,
}

/// Workspace members and root manifest at one commit.
#[derive(Clone, Debug)]
struct CommitSnapshot {
    packages: BTreeMap<String, HistoricalPackage>,
    root_doc: DocumentMut,
}

/// Cache of [`CommitSnapshot`] values so a first-parent walk does not re-parse.
struct SnapshotCache {
    inner: HashMap<String, Rc<CommitSnapshot>>,
    case: PathCase,
}

impl SnapshotCache {
    /// Probes the work tree once; every snapshot matches members the same way.
    fn new(workspace_root: &Path) -> Self {
        Self {
            inner: HashMap::new(),
            case: PathCase::probe(workspace_root),
        }
    }

    fn snapshot(&mut self, git: &GitRepo, commit: &str) -> Result<Rc<CommitSnapshot>, AppError> {
        if let Some(existing) = self.inner.get(commit) {
            return Ok(Rc::clone(existing));
        }
        let built = Rc::new(load_snapshot(git, commit, self.case)?);
        self.inner.insert(commit.to_string(), Rc::clone(&built));
        Ok(built)
    }
}

fn load_snapshot(git: &GitRepo, commit: &str, case: PathCase) -> Result<CommitSnapshot, AppError> {
    let root_rel = root_manifest_rel(git);
    // History before the workspace existed has no root manifest. An empty
    // `[workspace]` reproduces that state exactly: no members, so every current
    // package is absent from the snapshot and classified as newly created.
    let root_content = git
        .show_file(commit, &root_rel)?
        .unwrap_or_else(|| "[workspace]\n".to_string());
    let root_doc = parse_document(Path::new(&root_rel), &root_content)?;
    let members = parse_workspace_members(&root_content, Path::new(&root_rel), case)?;
    // `members` globs are written relative to the workspace root, which need not be
    // the git root, while `ls_tree_manifests` yields git-root-relative paths. Rebase
    // before matching, or a nested workspace would find no members and silently
    // classify every package as absent from the base revision.
    let workspace_prefix = root_rel.strip_suffix("Cargo.toml").unwrap_or("");
    let workspace = WorkspaceInherit::from_root(&root_doc);

    let mut manifest_paths: BTreeMap<String, String> = BTreeMap::new();
    for path in git.ls_tree_manifests(commit)? {
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let Some(member_dir) = workspace_relative_dir(dir, workspace_prefix) else {
            continue;
        };
        manifest_paths.insert(member_dir.to_string(), path);
    }

    let mut manifests = GitManifestSource {
        git,
        commit,
        workspace,
        paths: manifest_paths,
        parsed: BTreeMap::new(),
    };
    let member_dirs = resolve_members(&mut manifests, &members)?;
    let mut packages = BTreeMap::new();
    for member_dir in &member_dirs {
        let Some(parsed) = manifests.manifest(member_dir)? else {
            continue;
        };
        if !parsed.publish {
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
            },
        );
    }
    Ok(CommitSnapshot { packages, root_doc })
}

/// Supplies historical member manifests to membership resolution.
///
/// Cargo loads only the manifests that can become members of the selected
/// workspace, so resolution pulls manifests through this abstraction instead of
/// parsing every `Cargo.toml` in the tree up front. An unrelated or excluded
/// nested workspace holding a manifest Cargo would never read must not be able
/// to fail classification. Ref: docs/implementation.md, "Classification".
trait ManifestSource {
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
struct GitManifestSource<'a> {
    git: &'a GitRepo,
    commit: &'a str,
    workspace: WorkspaceInherit<'a>,
    /// Git-root-relative manifest path of every candidate directory.
    paths: BTreeMap<String, String>,
    parsed: BTreeMap<String, Option<PackageManifest>>,
}

impl ManifestSource for GitManifestSource<'_> {
    fn candidate_dirs(&self) -> Vec<String> {
        self.paths.keys().cloned().collect()
    }

    fn manifest(&mut self, dir: &str) -> Result<Option<&PackageManifest>, AppError> {
        if !self.parsed.contains_key(dir) {
            let parsed = match self.paths.get(dir) {
                Some(path) => match self.git.show_file(self.commit, path)? {
                    Some(content) => parse_package_manifest(&content, path, &self.workspace)?,
                    None => None,
                },
                None => None,
            };
            self.parsed.insert(dir.to_string(), parsed);
        }
        Ok(self.parsed.get(dir).and_then(Option::as_ref))
    }
}

/// Reconstructs the workspace-relative directories Cargo would treat as members.
///
/// Beyond the declared `members` patterns, Cargo makes every path dependency of
/// a member that lives inside the workspace a member too, so the closure is
/// followed until it stops growing. Membership derived this way still honours
/// `exclude`.
fn resolve_members(
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
    let relative = relative.replace('\\', "/");
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

// Untracked paths are advisory-only; tests cannot observe that a log line was skipped.
#[cfg_attr(test, mutants::skip)]
fn log_untracked(verbose: Verbose, name: &str, count: usize) {
    if count == 0 {
        return;
    }
    verbose.note(format!(
        "{name}: {} match packaging rules and are advisory only; released content is defined as \
         the git-tracked files under the package, so untracked paths are never counted as changes \
         even where Cargo would pack them",
        plural(count, "untracked path")
    ));
}

// Early-exit is equivalent to walking the rest of first-parent history.
#[cfg_attr(test, mutants::skip)]
fn can_stop_timeline(timeline: &[TimelineEntry]) -> bool {
    // The anchor walk starts at the newest commit that carried the package: the
    // base itself when the base carries it, the reintroduction point otherwise.
    // Until an older commit declares a different version the anchor is still
    // undetermined, and a package no commit has carried yet needs the whole
    // history to tell creation from truncation.
    let Some(reference) = timeline.iter().find_map(|entry| entry.version.as_ref()) else {
        return false;
    };
    let Some(last) = timeline.last() else {
        return false;
    };
    last.version.as_ref() != Some(reference)
}

/// Reads a tracked work-tree path the way Git stores its blob.
///
/// A symbolic link yields its target path rather than the content it points at,
/// because Git records a link as a blob holding the target. Dereferencing would
/// disagree with the anchor side, which is read out of the object database, and
/// would let a link committed under a package copy an arbitrary host file into
/// the generated patch.
fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            let target =
                fs::read_link(path).map_err(|error| ReadFileError::caused_by(path, error))?;
            // Git spells link targets with forward slashes regardless of platform.
            return Ok(Some(
                target.to_string_lossy().replace('\\', "/").into_bytes(),
            ));
        }
        Ok(_) => {}
        Err(error) if is_not_found(&error) => return Ok(None),
        Err(error) => return Err(ReadFileError::caused_by(path, error).into()),
    }
    match fs::read(path) {
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
/// docs/implementation.md, "Classification".
fn root_manifest_rel(git: &GitRepo) -> String {
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
/// separator, or empty when the workspace root is the git root. Returns `None` for
/// paths outside the workspace root, which belong to no member of this workspace.
fn workspace_relative_dir<'a>(dir: &'a str, workspace_prefix: &str) -> Option<&'a str> {
    if workspace_prefix.is_empty() {
        return Some(dir);
    }
    if dir == workspace_prefix.trim_end_matches('/') {
        return Some("");
    }
    dir.strip_prefix(workspace_prefix)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::inherited::InheritedKeys;

    #[test]
    fn workspace_relative_dir_rebases_onto_the_workspace_root() {
        // Workspace root is the git root: paths pass through unchanged.
        assert_eq!(workspace_relative_dir("packages/a", ""), Some("packages/a"));
        assert_eq!(workspace_relative_dir("", ""), Some(""));

        // Workspace root is nested: the prefix is stripped so member globs match.
        assert_eq!(
            workspace_relative_dir("rust/packages/a", "rust/"),
            Some("packages/a")
        );
        assert_eq!(workspace_relative_dir("rust", "rust/"), Some(""));

        // Outside the nested workspace root, so not a member of this workspace.
        assert_eq!(workspace_relative_dir("dotnet/packages/a", "rust/"), None);
        assert_eq!(workspace_relative_dir("", "rust/"), None);
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

    #[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
    #[test]
    fn read_optional_bytes_missing_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gone.txt");
        assert_eq!(read_optional_bytes(&path).unwrap(), None);
    }

    #[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
    #[test]
    fn read_optional_bytes_reads_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("here.txt");
        fs::write(&path, "hi").unwrap();
        assert_eq!(
            read_optional_bytes(&path).unwrap().as_deref(),
            Some(b"hi".as_slice())
        );
    }

    #[cfg_attr(miri, ignore)] // tempfile::tempdir is host filesystem, which Miri cannot emulate.
    #[test]
    fn read_optional_bytes_rejects_non_not_found_errors() {
        let dir = tempfile::tempdir().unwrap();
        let _ = read_optional_bytes(dir.path()).unwrap_err();
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
        // Windows-style separators appear in manifests authored on Windows.
        assert_eq!(
            join_relative("packages/a", r"..\b"),
            Some("packages/b".to_string())
        );
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
            nested_package_dirs(&paths, "packages/a"),
            vec!["packages/a/inner".to_string()]
        );
        assert_eq!(
            nested_package_dirs(&paths, "packages/a/inner"),
            Vec::<String>::new()
        );
        // A root package contains every other manifest.
        assert_eq!(
            nested_package_dirs(&paths, ""),
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
            nested_package_dirs(&paths, "packages/a"),
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
        };
        // `fixture` is a crate of its own, so Cargo packs none of its files with
        // `packages/a` even though the workspace never lists it as a member.
        let paths = vec![
            "packages/a/Cargo.toml".to_string(),
            "packages/a/src/lib.rs".to_string(),
            "packages/a/fixture/Cargo.toml".to_string(),
            "packages/a/fixture/src/lib.rs".to_string(),
        ];
        let released = released_from_paths(&paths, &side);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", "src/lib.rs"])
        );
        assert_eq!(released.get("Cargo.toml").unwrap(), "packages/a/Cargo.toml");
    }

    /// Git's `-z` listings separate directories with `/` on every platform, so a
    /// `\` in a reported path belongs to a file's name. Rewriting it would file
    /// the content under a directory that does not exist and, worse, could make
    /// two distinct files collide on one package-relative key.
    #[test]
    fn a_backslash_in_a_reported_path_is_not_a_directory_boundary() {
        let rules = PackagingRules::default();
        let resources = BTreeMap::new();
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
            resources: &resources,
            auto_readme: false,
        };
        let paths = vec![
            "packages/a/Cargo.toml".to_string(),
            r"packages/a/src/odd\name.rs".to_string(),
        ];
        let released = released_from_paths(&paths, &side);
        assert_eq!(
            released.keys().map(String::as_str).collect::<BTreeSet<_>>(),
            BTreeSet::from(["Cargo.toml", r"src/odd\name.rs"])
        );
        assert_eq!(
            released.get(r"src/odd\name.rs").unwrap(),
            r"packages/a/src/odd\name.rs"
        );
    }

    /// Cargo packs the README it detects itself even when `include` omits it, and
    /// prefers the first of its default names that the end being examined holds.
    #[test]
    fn a_detected_readme_outranks_the_packaging_rules() {
        let rules = PackagingRules::new(Some(&["src/**".to_string()]), None).unwrap();
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
        };

        let released = released_from_paths(&paths, &side);
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
            released_from_paths(&paths, &declared)
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["src/lib.rs"])
        );
    }

    /// A resource outside the package directory is released content under the
    /// name it takes at the crate root; one already inside keeps its own path.
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

    /// A nested workspace declares inherited resources relative to its own root,
    /// not to the git root.
    #[test]
    fn inherited_resources_resolve_against_the_workspace_prefix() {
        let manifest = manifest_with_resources("inner/packages/a", &[], &["README.md"]);

        let resolved = resolve_resources(&manifest, "inner/packages/a", "inner/");

        assert_eq!(
            resolved,
            BTreeMap::from([("README.md".to_string(), "inner/README.md".to_string())])
        );
    }

    /// A path climbing above the git root names no file in the repository, so it
    /// contributes no released content rather than failing classification.
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
            resource_paths: local.iter().map(|path| (*path).to_string()).collect(),
            inherited_resource_paths: inherited.iter().map(|path| (*path).to_string()).collect(),
            auto_readme: false,
        }
    }

    #[test]
    fn can_stop_timeline_waits_for_a_change_from_the_newest_carried_version() {
        fn entry(commit: &str, version: Option<&str>) -> TimelineEntry {
            TimelineEntry {
                commit: commit.to_string(),
                version: version.map(|text| text.parse().unwrap()),
                has_parent: true,
            }
        }

        assert!(!can_stop_timeline(&[entry("c2", Some("0.1.0"))]));
        assert!(can_stop_timeline(&[
            entry("c2", Some("0.1.0")),
            entry("c1", Some("0.0.9")),
        ]));
        // Absent at the base and never carried: creation cannot be told from
        // truncation until the walk reaches a root.
        assert!(!can_stop_timeline(&[entry("c2", None), entry("c1", None)]));
        // Absent at the base but carried earlier: the reintroduction anchor is
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
    }

    #[test]
    fn is_inside_any_requires_a_directory_boundary() {
        let dirs = vec!["packages/a".to_string()];
        assert!(is_inside_any("packages/a/src/lib.rs", &dirs));
        assert!(!is_inside_any("packages/a", &dirs));
        assert!(!is_inside_any("packages/ab/src/lib.rs", &dirs));
    }

    /// Serves pre-parsed manifests so membership resolution can be exercised
    /// without a repository.
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
            "[workspace]\nmembers = [\"packages/a\"]\nexclude = [\"packages/c\"]\n",
            root,
            PathCase::Sensitive,
        )
        .unwrap();
        let mut manifests = FakeManifests::new(&[
            (
                "packages/a",
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nb = { path = \"../b\" }\n\n[dev-dependencies]\nc = { path = \"../c\" }\n",
            ),
            (
                "packages/b",
                "[package]\nname = \"b\"\nversion = \"0.1.0\"\n",
            ),
            (
                "packages/c",
                "[package]\nname = \"c\"\nversion = \"0.1.0\"\n",
            ),
        ]);
        let resolved = resolve_members(&mut manifests, &members).unwrap();
        // `b` is reachable only as a path dependency; `c` is excluded even though
        // a member depends on it.
        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["packages/a".to_string(), "packages/b".to_string()]
        );
    }

    /// A manifest Cargo would never load for this workspace is never parsed, so
    /// an unrelated nested workspace cannot fail classification.
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

    /// A path dependency that climbs out of the repository is not a workspace
    /// member, and following it would name a directory outside the tree.
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
