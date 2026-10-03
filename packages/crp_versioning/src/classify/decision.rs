//! Closed classification inputs and relocatable decisions, separate from live acquisition.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::rc::Rc;

use crp_diag::Verbose;
use crp_workspace::cache::{Cache, CacheEntry};
use crp_workspace::git::{BLOB_BATCH_BYTES, BlobReader, GitObjectContext};
use crp_workspace::lockfile::{
    Closure, ClosureChange, InstallationGraph, Lockfile, closure_changes,
};
use crp_workspace::manifest::PathCase;
use crp_workspace::manifest_document::ManifestDocument;
use crp_workspace::metadata::{DepKind, ReportedDep, dependents_of};
use ohno::AppError;
use semver::Version;
use serde::ser::Error as _;
use serde::{Deserialize, Serialize, Serializer};

use crate::anchor::Anchor;
use crate::classify::{
    ChangedItem, Classification, DiffStat, PackageClass, ReleasedFiles, Verdict, required_closure,
};
use crate::groups::{GroupVerdict, Groups};
use crate::inherited::InheritedInputs;

/// The complete acquired model consumed by decision computation.
///
/// No workspace, repository handle or path to a live file crosses this boundary. Object reads
/// for rendering are the sole effect, constrained to exact IDs already present in these inputs.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct DecisionInputs {
    pub(crate) objects: GitObjectContext,
    pub(crate) case: PathCase,
    pub(crate) prefix: String,
    pub(crate) release_history: String,
    pub(crate) merge_target: Option<String>,
    pub(crate) versions: BTreeMap<String, Version>,
    pub(crate) exact_dependencies: Vec<(String, String, String, String, String)>,
    pub(crate) exempt: BTreeSet<String>,
    pub(crate) locks: BTreeMap<String, LockInputs>,
    pub(crate) packages: Vec<PackageInputs>,
}

impl DecisionInputs {
    fn key(&self) -> Result<Option<String>, AppError> {
        if !self.objects.portable()
            || self
                .locks
                .values()
                .any(|lock| lock.installation.cache_input().is_none())
        {
            return Ok(None);
        }
        Ok(Some(serde_json::to_string(&(
            env!("CARGO_PKG_VERSION"),
            Decisions::REVISION,
            self,
        ))?))
    }

    /// Computes policy solely from the acquired model and immutable rendering objects.
    pub(crate) fn compute(
        &self,
        mut sizes: impl FnMut(&[&str]) -> Result<Vec<usize>, AppError>,
        mut blobs: impl FnMut(&[&str]) -> Result<Vec<Vec<u8>>, AppError>,
    ) -> Result<Decisions, AppError> {
        let membership = Groups::from_edges(
            self.versions.keys().cloned(),
            self.exact_dependencies
                .iter()
                .map(|(source, target, ..)| (source.clone(), target.clone())),
        );
        let exempt: HashSet<_> = self.exempt.iter().cloned().collect();
        let groups = membership.verdicts(&self.versions, &exempt);
        let packages =
            self.packages
                .iter()
                .map(|package| {
                    let (verdict, stat) = match &package.anchor {
                        None => (
                            Verdict::New,
                            DiffStat {
                                files: 0,
                                insertions: 0,
                                deletions: 0,
                            },
                        ),
                        Some(anchor) => {
                            let identified = anchor.files.identify();
                            let ids = identified.blob_ids();
                            let mut reader = BlobReader::new(&ids, BLOB_BATCH_BYTES, &mut sizes)?;
                            let (mut changed, patch, stat) =
                                identified.render(|id| reader.read(id, &mut blobs))?;
                            changed.extend(
                                anchor
                                    .inherited
                                    .changes()
                                    .into_iter()
                                    .map(|item| ChangedItem::Inherited { field: item.field }),
                            );
                            changed.extend(
                                lock_changes(
                                    &self.locks,
                                    anchor.old_lock.as_deref(),
                                    anchor.new_lock.as_deref(),
                                    &package.name,
                                    &anchor.anchor.version,
                                    &package.version,
                                )?
                                .into_iter()
                                .map(|(dependency, change)| ChangedItem::Lockfile {
                                    dependency,
                                    change: change.as_str().to_owned(),
                                }),
                            );
                            (
                                Verdict::anchored(
                                    &package.name,
                                    &package.version,
                                    anchor.anchor.clone(),
                                    changed,
                                    patch,
                                )?,
                                stat,
                            )
                        }
                    };
                    Ok((package.name.clone(), (verdict, stat)))
                })
                .collect::<Result<_, AppError>>()?;
        Ok(Decisions {
            packages,
            membership,
            groups,
        })
    }
}

/// One package's acquired facts; output paths and live metadata stay in the fresh envelope.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct PackageInputs {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) directory: String,
    pub(crate) manifest: ManifestDocument,
    // Include the dependency kind explicitly: ReportedDep's public JSON intentionally omits it.
    pub(crate) dependencies: Vec<(ReportedDep, DepKind)>,
    pub(crate) consumer_contract: bool,
    pub(crate) resources: BTreeMap<String, String>,
    pub(crate) auto_readme: bool,
    pub(crate) untracked: Vec<String>,
    pub(crate) anchor: Option<AnchorInputs>,
}

/// Endpoint observations needed after fresh anchor selection and content acquisition.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct AnchorInputs {
    pub(crate) anchor: Anchor,
    pub(crate) files: ReleasedFiles,
    pub(crate) inherited: InheritedInputs,
    pub(crate) old_lock: Option<String>,
    pub(crate) new_lock: Option<String>,
}

/// A lock graph and its independently acquired installation interpretation.
///
/// Deferred errors remain live. They disable reuse, not classification of unrelated packages.
#[derive(Clone, Debug)]
pub(crate) struct LockInputs {
    pub(crate) lockfile: Rc<Lockfile>,
    pub(crate) installation: InstallationGraph,
}

impl LockInputs {
    fn closure(&self, name: &str, version: &Version, reason: &str) -> Result<Closure, AppError> {
        required_closure(
            self.lockfile
                .closure(name, &version.to_string(), &self.installation)?,
            name,
            reason,
        )
    }
}

impl Serialize for LockInputs {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let installation = self.installation.cache_input().ok_or_else(|| {
            S::Error::custom("deferred installation errors are not reusable inputs")
        })?;
        (self.lockfile.as_ref(), installation).serialize(serializer)
    }
}

/// Compares independently optional binary endpoints against their shared acquired graphs.
pub(crate) fn lock_changes(
    locks: &BTreeMap<String, LockInputs>,
    old: Option<&str>,
    new: Option<&str>,
    name: &str,
    old_version: &Version,
    new_version: &Version,
) -> Result<Vec<(String, ClosureChange)>, AppError> {
    let closure = |key: Option<&str>, version: &Version, reason: &str| {
        key.map(|key| {
            locks
                .get(key)
                .expect("every selected lock endpoint was acquired")
                .closure(name, version, reason)
        })
        .transpose()
        .map(Option::unwrap_or_default)
    };
    let old = closure(
        old,
        old_version,
        "the anchor Cargo.lock does not identify an installation closure at the declared version and configured sources",
    )?;
    let new = closure(
        new,
        new_version,
        "the work-tree Cargo.lock does not identify an installation closure at the declared version and configured sources; refresh Cargo.lock",
    )?;
    Ok(closure_changes(&old, &new))
}

/// Derived policy and rendered evidence only; never stores a Classification or admission verdict.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Decisions {
    packages: BTreeMap<String, (Verdict, DiffStat)>,
    membership: Groups,
    groups: BTreeMap<String, GroupVerdict>,
}

impl CacheEntry for Decisions {
    const SUBJECT: &'static str = "classification-decisions";
    // Bump when acquisition normalization, policy or rendering semantics change within a release.
    const REVISION: u32 = 1;
    type Key = String;
}

impl Decisions {
    pub(crate) fn apply(self, inputs: &DecisionInputs, current: &mut Classification) {
        current.packages = inputs
            .packages
            .iter()
            .zip(&current.work_tree.packages)
            .map(|(input, package)| {
                let (verdict, stat) = self
                    .packages
                    .get(&input.name)
                    .expect("an admitted decision was computed for every input package");
                PackageClass {
                    name: input.name.clone(),
                    declared_version: package.manifest.version.clone(),
                    group: self.membership.group_of(&input.name).map(str::to_owned),
                    verdict: verdict.clone(),
                    stat: stat.clone(),
                    untracked: input.untracked.clone(),
                    dependencies: package.dependencies.clone(),
                    dependents: dependents_of(&current.work_tree.packages, &input.name),
                    consumer_contract: package.consumer_contract,
                    manifest_path: package.manifest_path.clone(),
                }
            })
            .collect();
        current.groups = self.groups;
        current.membership = self.membership;
    }
}

/// At most the preceding complete input and decision, shared across explicit fresh passes.
#[derive(Debug, Default)]
pub(crate) struct DecisionCache {
    last: Option<(String, Decisions)>,
}

impl DecisionCache {
    pub(crate) fn get(
        &mut self,
        input: &DecisionInputs,
        storage: &Cache,
        verbose: Verbose<'_>,
        compute: impl FnOnce() -> Result<Decisions, AppError>,
    ) -> Result<Decisions, AppError> {
        let key = if storage.directory().is_some() {
            input.key()?
        } else {
            None
        };
        self.get_with(
            key,
            verbose,
            |key, compute| storage.get(key, verbose, compute),
            compute,
        )
    }

    fn get_with(
        &mut self,
        key: Option<String>,
        verbose: Verbose<'_>,
        load: impl FnOnce(
            &String,
            &mut dyn FnMut() -> Result<Decisions, AppError>,
        ) -> Result<Decisions, AppError>,
        compute: impl FnOnce() -> Result<Decisions, AppError>,
    ) -> Result<Decisions, AppError> {
        let Some(key) = key else {
            verbose.note(|| "computing classification decisions because storage is disabled or acquired interpretation is not reusable".to_owned());
            return compute();
        };
        if let Some((previous, decisions)) = &self.last
            && previous == &key
        {
            verbose.note(|| "reusing classification decisions from memory because complete freshly acquired inputs are equal".to_owned());
            return Ok(decisions.clone());
        }
        let mut compute = Some(compute);
        let mut computed = false;
        let value = load(&key, &mut || {
            computed = true;
            compute
                .take()
                .expect("the cache invokes computation at most once")()
        })?;
        verbose.note(|| if computed {
            "computed classification decisions because no compatible entry matches complete freshly acquired inputs".to_owned()
        } else {
            "reusing classification decisions from storage because complete freshly acquired inputs and computation revision match".to_owned()
        });
        self.last = Some((key, value.clone()));
        Ok(value)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
