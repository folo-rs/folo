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
use crate::git::{GitRepo, git_path, join_git_rel};
use crate::groups::GroupVerdict;
use crate::inherited::{InheritedChange, inherited_changes};
use crate::manifest::{
    PackageManifest, PathCase, WorkspaceInherit, WorkspaceMembers, is_workspace_excluded,
    is_workspace_member, parse_document, parse_package_manifest, parse_workspace_members,
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
        if class.anchor.is_none() {
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
                    untracked: Vec::new(),
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
        },
        &PackageSide {
            dir: &package.manifest.directory,
            rules: &package.manifest.packaging,
        },
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
}

fn diff_package(
    git: &GitRepo,
    anchor_commit: &str,
    anchor: &PackageSide<'_>,
    work: &PackageSide<'_>,
) -> Result<(Vec<ChangedItem>, String, DiffStat, Vec<String>), AppError> {
    let anchor_files = released_at_commit(git, anchor_commit, anchor)?;
    let work_files = released_in_work_tree(git, work)?;

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

    let untracked = git
        .ls_untracked(work.dir)?
        .into_iter()
        .filter_map(|full| {
            let rel = relativize(&git_path(&full), work.dir)?.to_string();
            work.rules.is_released(&rel).then_some(rel)
        })
        .collect();

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
    Ok(released_from_paths(&git.ls_tree(commit, side.dir)?, side))
}

fn released_in_work_tree(
    git: &GitRepo,
    side: &PackageSide<'_>,
) -> Result<HashMap<String, String>, AppError> {
    Ok(released_from_paths(&git.ls_files(side.dir)?, side))
}

/// Selects one package's released content from the tracked paths beneath it.
///
/// Keys are package-relative, values are the git-root-relative paths the caller
/// reads the content back from.
fn released_from_paths(paths: &[String], side: &PackageSide<'_>) -> HashMap<String, String> {
    let paths: Vec<String> = paths.iter().map(|path| git_path(path)).collect();
    let nested = nested_package_dirs(&paths, side.dir);
    let mut map = HashMap::new();
    for full in paths {
        if is_inside_any(&full, &nested) {
            continue;
        }
        let Some(rel) = relativize(&full, side.dir) else {
            continue;
        };
        if side.rules.is_released(rel) {
            let rel = rel.to_string();
            map.insert(rel, full);
        }
    }
    map
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
struct HistoricalPackage {
    directory: String,
    version: Version,
    packaging: PackagingRules,
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

    let mut candidates: BTreeMap<String, PackageManifest> = BTreeMap::new();
    for path in git.ls_tree_manifests(commit)? {
        let path = git_path(&path);
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let Some(member_dir) = workspace_relative_dir(dir, workspace_prefix) else {
            continue;
        };
        let Some(content) = git.show_file(commit, &path)? else {
            continue;
        };
        let Some(parsed) = parse_package_manifest(&content, &path, &workspace)? else {
            continue;
        };
        candidates.insert(member_dir.to_string(), parsed);
    }

    let member_dirs = resolve_members(&candidates, &members);
    let mut packages = BTreeMap::new();
    for member_dir in &member_dirs {
        let Some(parsed) = candidates.get(member_dir) else {
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
            },
        );
    }
    Ok(CommitSnapshot { packages, root_doc })
}

/// Reconstructs the workspace-relative directories Cargo would treat as members.
///
/// Beyond the declared `members` patterns, Cargo makes every path dependency of
/// a member that lives inside the workspace a member too, so the closure is
/// followed until it stops growing. Membership derived this way still honours
/// `exclude`.
fn resolve_members(
    candidates: &BTreeMap<String, PackageManifest>,
    members: &WorkspaceMembers,
) -> BTreeSet<String> {
    let mut resolved: BTreeSet<String> = candidates
        .keys()
        .filter(|dir| is_workspace_member(dir, members))
        .cloned()
        .collect();
    let mut pending: Vec<String> = resolved.iter().cloned().collect();
    while let Some(dir) = pending.pop() {
        let Some(parsed) = candidates.get(&dir) else {
            continue;
        };
        // Locally declared paths resolve against the member's own directory;
        // inherited ones are declared in `[workspace.dependencies]` and so
        // resolve against the workspace root.
        let edges = parsed
            .path_dependencies
            .iter()
            .map(|relative| (dir.as_str(), relative))
            .chain(
                parsed
                    .inherited_path_dependencies
                    .iter()
                    .map(|relative| ("", relative)),
            );
        for (base, relative) in edges {
            let Some(target) = join_relative(base, relative) else {
                continue;
            };
            if !candidates.contains_key(&target) || is_workspace_excluded(&target, members) {
                continue;
            }
            if resolved.insert(target.clone()) {
                pending.push(target);
            }
        }
    }
    resolved
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
        "{name}: {} match packaging rules and are advisory only; \
         Cargo would not put them in the .crate so they are not counted as changes",
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

fn read_optional_bytes(path: &Path) -> Result<Option<Vec<u8>>, AppError> {
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
        let side = PackageSide {
            dir: "packages/a",
            rules: &rules,
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

    #[test]
    fn resolve_members_follows_path_dependencies() {
        let root = Path::new("Cargo.toml");
        let members = parse_workspace_members(
            "[workspace]\nmembers = [\"packages/a\"]\nexclude = [\"packages/c\"]\n",
            root,
            PathCase::Sensitive,
        )
        .unwrap();
        let mut candidates = BTreeMap::new();
        candidates.insert(
            "packages/a".to_string(),
            parse_package_manifest(
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nb = { path = \"../b\" }\n\n[dev-dependencies]\nc = { path = \"../c\" }\n",
                "packages/a/Cargo.toml",
                &WorkspaceInherit::default(),
            )
            .unwrap()
            .unwrap(),
        );
        candidates.insert(
            "packages/b".to_string(),
            parse_package_manifest(
                "[package]\nname = \"b\"\nversion = \"0.1.0\"\n",
                "packages/b/Cargo.toml",
                &WorkspaceInherit::default(),
            )
            .unwrap()
            .unwrap(),
        );
        candidates.insert(
            "packages/c".to_string(),
            parse_package_manifest(
                "[package]\nname = \"c\"\nversion = \"0.1.0\"\n",
                "packages/c/Cargo.toml",
                &WorkspaceInherit::default(),
            )
            .unwrap()
            .unwrap(),
        );
        let resolved = resolve_members(&candidates, &members);
        // `b` is reachable only as a path dependency; `c` is excluded even though
        // a member depends on it.
        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["packages/a".to_string(), "packages/b".to_string()]
        );
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
        let mut candidates = BTreeMap::new();
        candidates.insert(
            "packages/a".to_string(),
            parse_package_manifest(
                "[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nout = { path = \"../../../outside\" }\n",
                "packages/a/Cargo.toml",
                &WorkspaceInherit::default(),
            )
            .unwrap()
            .unwrap(),
        );

        let resolved = resolve_members(&candidates, &members);

        assert_eq!(
            resolved.into_iter().collect::<Vec<_>>(),
            vec!["packages/a".to_string()]
        );
    }
}
