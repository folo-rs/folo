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

use crate::anchor::{Anchor, TimelineEntry, resolve_anchor};
use crate::git::{GitRepo, git_path, join_git_rel};
use crate::groups::GroupVerdict;
use crate::inherited::{InheritedChange, inherited_changes, workspace_package_version};
use crate::manifest::{
    PathCase, is_workspace_member, parse_document, parse_package_manifest, parse_workspace_members,
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
        package.manifest.directory = join_git_rel(
            git.root(),
            &work_tree.workspace_root,
            &package.manifest.directory,
        );
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
    let base_snapshot = cache.snapshot(&git, &base_sha, &work_tree.workspace_root)?;
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

    if !base_snapshot.packages.contains_key(name) {
        verbose.note(format!(
            "{name}: absent from base {base_sha}, so creation on this branch counts as a \
             version increase and the status is releasing"
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

    let timeline = build_timeline(git, name, commits, cache, &work_tree.workspace_root)?;
    let anchor = resolve_anchor(name, &timeline)?;
    let short_anchor = short_commit(&anchor.commit);
    verbose.note(format!(
        "{name}: anchor {short_anchor} declared {}; work tree declares {}; a status of releasing \
         requires the work-tree version to be greater than the anchor version (parsed, not textual)",
        anchor.version,
        package.manifest.version
    ));

    let anchor_snapshot = cache.snapshot(git, &anchor.commit, &work_tree.workspace_root)?;
    let Some(anchor_pkg) = anchor_snapshot.packages.get(name) else {
        verbose.note(format!(
            "{name}: package is missing at its own anchor {}; treating that as a version increase",
            anchor.commit
        ));
        return Ok(PackageClass {
            name: name.clone(),
            declared_version: package.manifest.version.clone(),
            group,
            status: PackageStatus::Releasing,
            anchor: Some(anchor),
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
    };

    let (changed_files, mut patch, stat, untracked) = diff_package(
        git,
        &anchor.commit,
        &anchor_pkg.directory,
        &anchor_pkg.packaging,
        &package.manifest.directory,
        &package.manifest.packaging,
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
    workspace_root: &Path,
) -> Result<Vec<TimelineEntry>, AppError> {
    let mut timeline = Vec::with_capacity(commits.len());
    for (index, commit) in commits.iter().enumerate() {
        let snapshot = cache.snapshot(git, commit, workspace_root)?;
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

fn diff_package(
    git: &GitRepo,
    anchor: &str,
    anchor_dir: &str,
    anchor_rules: &PackagingRules,
    work_dir: &str,
    work_rules: &PackagingRules,
) -> Result<(Vec<ChangedItem>, String, DiffStat, Vec<String>), AppError> {
    let anchor_files = released_at_commit(git, anchor, anchor_dir, anchor_rules)?;
    let work_files = released_in_work_tree(git, work_dir, work_rules)?;

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
            Some(path) => git.show_file_bytes(anchor, path)?,
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
        let (file_patch, ins, del) = file_diff(rel, old.as_deref(), new.as_deref());
        insertions = insertions.saturating_add(ins);
        deletions = deletions.saturating_add(del);
        patch.push_str(&file_patch);
    }

    let untracked = git
        .ls_untracked(work_dir)?
        .into_iter()
        .filter_map(|full| {
            let rel = relativize(&git_path(&full), work_dir)?.to_string();
            work_rules.is_released(&rel).then_some(rel)
        })
        .collect();

    let stat = DiffStat {
        files: changed.len(),
        insertions,
        deletions,
    };
    Ok((changed, patch, stat, untracked))
}

fn file_diff(path: &str, old: Option<&[u8]>, new: Option<&[u8]>) -> (String, usize, usize) {
    let (old_present, new_present) = (old.is_some(), new.is_some());
    // A unified diff can only describe text, and validating here lets both sides
    // be borrowed rather than copied out of a lossy conversion.
    let (Ok(old), Ok(new)) = (
        str::from_utf8(old.unwrap_or_default()),
        str::from_utf8(new.unwrap_or_default()),
    ) else {
        return (format!("Binary files a/{path} and b/{path} differ\n"), 0, 0);
    };
    unified_diff(path, old, old_present, new, new_present)
}

/// Renders one file's change as a zero-context unified diff hunk.
///
/// Consumers pipe `.patch` artifacts into standard tooling, so the output
/// follows the unified format that `diff -U0` produces: `/dev/null` for the
/// absent side of an addition or deletion, and a hunk header whose line numbers
/// use the preceding line when a side contributes no lines.
fn unified_diff(
    path: &str,
    old: &str,
    old_present: bool,
    new: &str,
    new_present: bool,
) -> (String, usize, usize) {
    // `split_inclusive` keeps line terminators, so a change that only adds or
    // removes a trailing newline is still visible as a differing line.
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let mut prefix = 0_usize;
    while let (Some(old_line), Some(new_line)) = (old_lines.get(prefix), new_lines.get(prefix)) {
        if old_line != new_line {
            break;
        }
        prefix = prefix.saturating_add(1);
    }
    let mut old_end = old_lines.len();
    let mut new_end = new_lines.len();
    while old_end > prefix && new_end > prefix {
        let Some(old_line) = old_lines.get(old_end.saturating_sub(1)) else {
            break;
        };
        let Some(new_line) = new_lines.get(new_end.saturating_sub(1)) else {
            break;
        };
        if old_line != new_line {
            break;
        }
        old_end = old_end.saturating_sub(1);
        new_end = new_end.saturating_sub(1);
    }
    let old_mid = old_lines.get(prefix..old_end).unwrap_or(&[]);
    let new_mid = new_lines.get(prefix..new_end).unwrap_or(&[]);
    let deletions = old_mid.len();
    let insertions = new_mid.len();
    if deletions == 0 && insertions == 0 {
        return (String::new(), 0, 0);
    }
    // A side that contributes no lines anchors on the preceding line, which is
    // what `diff -U0` emits and what patch readers expect.
    let old_start = if deletions == 0 {
        prefix
    } else {
        prefix.saturating_add(1)
    };
    let new_start = if insertions == 0 {
        prefix
    } else {
        prefix.saturating_add(1)
    };
    let old_label = side_label(old_present, "a", path);
    let new_label = side_label(new_present, "b", path);
    let mut out = format!(
        "--- {old_label}\n+++ {new_label}\n\
         @@ -{old_start},{deletions} +{new_start},{insertions} @@\n"
    );
    for line in old_mid {
        push_diff_line(&mut out, '-', line);
    }
    for line in new_mid {
        push_diff_line(&mut out, '+', line);
    }
    (out, insertions, deletions)
}

fn side_label(present: bool, prefix: &str, path: &str) -> String {
    if present {
        format!("{prefix}/{path}")
    } else {
        // The unified-diff placeholder for the absent side of an add or delete.
        "/dev/null".to_string()
    }
}

fn push_diff_line(out: &mut String, marker: char, line: &str) {
    out.push(marker);
    out.push_str(line);
    if !line.ends_with('\n') {
        out.push_str("\n\\ No newline at end of file\n");
    }
}

fn released_at_commit(
    git: &GitRepo,
    commit: &str,
    dir: &str,
    rules: &PackagingRules,
) -> Result<HashMap<String, String>, AppError> {
    let mut map = HashMap::new();
    let pathspec = if dir.is_empty() { "." } else { dir };
    for full in git.ls_tree(commit, pathspec)? {
        let full = git_path(&full);
        let Some(rel) = relativize(&full, dir) else {
            continue;
        };
        if rules.is_released(rel) {
            map.insert(rel.to_string(), full);
        }
    }
    Ok(map)
}

fn released_in_work_tree(
    git: &GitRepo,
    dir: &str,
    rules: &PackagingRules,
) -> Result<HashMap<String, String>, AppError> {
    let mut map = HashMap::new();
    let pathspec = if dir.is_empty() { "." } else { dir };
    for full in git.ls_files(pathspec)? {
        let full = git_path(&full);
        let Some(rel) = relativize(&full, dir) else {
            continue;
        };
        if rules.is_released(rel) {
            map.insert(rel.to_string(), full);
        }
    }
    Ok(map)
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

    fn snapshot(
        &mut self,
        git: &GitRepo,
        commit: &str,
        workspace_root: &Path,
    ) -> Result<Rc<CommitSnapshot>, AppError> {
        if let Some(existing) = self.inner.get(commit) {
            return Ok(Rc::clone(existing));
        }
        let built = Rc::new(load_snapshot(git, commit, workspace_root, self.case)?);
        self.inner.insert(commit.to_string(), Rc::clone(&built));
        Ok(built)
    }
}

fn load_snapshot(
    git: &GitRepo,
    commit: &str,
    workspace_root: &Path,
    case: PathCase,
) -> Result<CommitSnapshot, AppError> {
    let root_rel = root_manifest_rel(git, workspace_root);
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
    let workspace_version = workspace_package_version(&root_doc);
    let mut packages = BTreeMap::new();
    for path in git.ls_tree_manifests(commit)? {
        let path = git_path(&path);
        let dir = path.rsplit_once('/').map_or("", |(dir, _)| dir);
        let Some(member_dir) = workspace_relative_dir(dir, workspace_prefix) else {
            continue;
        };
        if !is_workspace_member(member_dir, &members) {
            continue;
        }
        let Some(content) = git.show_file(commit, &path)? else {
            continue;
        };
        let Some(parsed) = parse_package_manifest(&content, &path, workspace_version)? else {
            continue;
        };
        if !parsed.publish {
            continue;
        }
        packages.insert(
            parsed.name.clone(),
            HistoricalPackage {
                directory: parsed.directory,
                version: parsed.version,
                packaging: parsed.packaging,
            },
        );
    }
    Ok(CommitSnapshot { packages, root_doc })
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
    if timeline.len() <= 1 {
        return false;
    }
    let first = timeline.first().and_then(|entry| entry.version.clone());
    let last = timeline.last().and_then(|entry| entry.version.clone());
    last != first
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

// Git-root-relative Cargo.toml path; only used to load historical root manifests.
#[cfg_attr(test, mutants::skip)]
fn root_manifest_rel(git: &GitRepo, workspace_root: &Path) -> String {
    let rel = workspace_root
        .strip_prefix(git.root())
        .unwrap_or_else(|_| Path::new(""));
    let path = rel.join("Cargo.toml");
    path.to_string_lossy()
        .replace('\\', "/")
        .trim_start_matches("./")
        .to_string()
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
    fn added_file_diff_uses_the_dev_null_placeholder() {
        let (patch, insertions, deletions) = file_diff("src/new.rs", None, Some(b"one\ntwo\n"));
        assert!(patch.starts_with("--- /dev/null\n+++ b/src/new.rs\n@@ -0,0 +1,2 @@\n"));
        assert_eq!((insertions, deletions), (2, 0));
    }

    #[test]
    fn deleted_file_diff_uses_the_dev_null_placeholder() {
        let (patch, insertions, deletions) = file_diff("src/old.rs", Some(b"one\n"), None);
        assert!(patch.starts_with("--- a/src/old.rs\n+++ /dev/null\n@@ -1,1 +0,0 @@\n"));
        assert_eq!((insertions, deletions), (0, 1));
    }

    #[test]
    fn a_missing_trailing_newline_is_a_visible_change() {
        let (patch, insertions, deletions) = file_diff("src/lib.rs", Some(b"one\n"), Some(b"one"));
        assert!(patch.contains("\\ No newline at end of file"));
        assert_eq!((insertions, deletions), (1, 1));
    }

    #[test]
    fn non_utf8_content_is_reported_as_binary() {
        let (patch, insertions, deletions) =
            file_diff("logo.png", Some(&[0x00, 0xff]), Some(&[0x00, 0xfe]));
        assert_eq!(patch, "Binary files a/logo.png and b/logo.png differ\n");
        assert_eq!((insertions, deletions), (0, 0));
    }

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
    fn unified_diff_hunks_changed_middle_only() {
        let (patch, insertions, deletions) = unified_diff(
            "src/lib.rs",
            "keep\nold\nend\n",
            true,
            "keep\nnew\nend\n",
            true,
        );
        assert!(patch.contains("@@ -2,1 +2,1 @@"));
        assert!(patch.contains("-old"));
        assert!(patch.contains("+new"));
        assert!(!patch.contains("-keep"));
        assert_eq!((insertions, deletions), (1, 1));
    }

    #[test]
    fn unified_diff_anchors_an_empty_side_on_the_preceding_line() {
        let (patch, insertions, deletions) = unified_diff("f.rs", "z\nz\n", true, "z\n", true);
        assert_eq!((insertions, deletions), (0, 1));
        assert!(patch.contains("@@ -2,1 +1,0 @@"));
        assert!(patch.contains("-z"));
        let (patch, insertions, deletions) = unified_diff("f.rs", "z\n", true, "z\nz\n", true);
        assert_eq!((insertions, deletions), (1, 0));
        assert!(patch.contains("@@ -1,0 +2,1 @@"));
        assert!(patch.contains("+z"));
    }

    #[test]
    fn file_diff_text_is_not_binary() {
        let (patch, insertions, deletions) =
            file_diff("src/lib.rs", Some(b"old\n"), Some(b"new\n"));
        assert!(!patch.contains("Binary files"));
        assert!(patch.contains("-old"));
        assert_eq!((insertions, deletions), (1, 1));
    }

    #[test]
    fn file_diff_one_sided_binary_is_binary() {
        let (patch, _, _) = file_diff("x.bin", Some(&[0xff]), Some(b"hello"));
        assert!(patch.contains("Binary files"));
        let (patch, _, _) = file_diff("x.bin", Some(b"hello"), Some(&[0xff]));
        assert!(patch.contains("Binary files"));
    }

    #[test]
    fn file_diff_reports_binary_without_utf8_replacement() {
        let (patch, insertions, deletions) =
            file_diff("icon.bin", Some(&[0xff, 0x00]), Some(&[0xfe, 0x00]));
        assert!(patch.contains("Binary files"));
        assert_eq!((insertions, deletions), (0, 0));
    }
}
