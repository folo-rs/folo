//! Shared admission and deletion policy for scoped and partition-local blessings.

use cbh_detect::{DiscriminantFilter, DiscriminantSetQuery};
use cbh_model::{BlessingRecord, BlessingScope, DiscriminantSet, ScopedBlessingRecord, StorageKey};
use cbh_storage::Storage;

use crate::load::load_objects_concurrently;
use crate::{
    AnalyzeError, BlessingScopeConflictError, InvalidBlessingError, InvalidStoredUtf8Error,
};

/// Key-only metadata allows topology filtering before record acquisition.
pub(crate) struct BlessingCandidate {
    pub(crate) commit: String,
    /// A partition-local record derives its scope exclusively from its key.
    pub(crate) legacy_set: Option<DiscriminantSet>,
}

/// A decoded blessing with its actual persisted scope, not the query's scope.
pub(crate) struct StoredBlessing {
    pub(crate) key: String,
    pub(crate) commit: String,
    pub(crate) scope: BlessingScope,
    pub(crate) record: BlessingRecord,
}

/// Combines key metadata without expanding logical scopes into observed partitions.
pub(crate) fn blessing_candidates(
    legacy: Vec<(String, StorageKey)>,
    mut scoped: Vec<(String, BlessingCandidate)>,
) -> Vec<(String, BlessingCandidate)> {
    scoped.extend(legacy.into_iter().map(|(key, parsed)| {
        (
            key,
            BlessingCandidate {
                commit: parsed.commit,
                legacy_set: Some(parsed.set),
            },
        )
    }));
    scoped
}

/// Captures resolved filters without their auto-detection provenance.
pub(crate) fn blessing_scope(query: &DiscriminantSetQuery) -> BlessingScope {
    /// Canonicalizes equivalent filter spellings for stable storage and audit output.
    fn values(filter: &DiscriminantFilter) -> Vec<String> {
        let mut values: Vec<_> = match filter {
            DiscriminantFilter::All => Vec::new(),
            DiscriminantFilter::Auto(value) => vec![value.to_ascii_lowercase()],
            DiscriminantFilter::Explicit(values) => values
                .iter()
                .map(|value| value.to_ascii_lowercase())
                .collect(),
        };
        values.sort();
        values.dedup();
        values
    }
    BlessingScope {
        engine: values(&query.engine),
        target_triple: values(&query.target_triple),
        machine_key: values(&query.machine_key),
    }
}

/// Loads both formats and retains scopes that intersect the selected partitions.
pub(crate) async fn load_blessings<S: Storage>(
    storage: &S,
    candidates: Vec<(String, BlessingCandidate)>,
    query: &DiscriminantSetQuery,
) -> Result<Vec<StoredBlessing>, AnalyzeError> {
    let mut fetched = load_objects_concurrently(storage, candidates, |key, candidate, bytes| {
        let text = String::from_utf8(bytes)
            .map_err(|error| InvalidStoredUtf8Error::caused_by("stored blessing", key, error))?;
        let (record, scope) = if let Some(set) = &candidate.legacy_set {
            BlessingRecord::from_json(&text).map(|record| (record, BlessingScope::from(set)))
        } else {
            ScopedBlessingRecord::from_json(&text).map(|scoped| (scoped.record, scoped.scope))
        }
        .map_err(|error| {
            InvalidBlessingError::caused_by("stored blessing", key, "blessing record", error)
        })?;
        if record.commit != candidate.commit {
            return Err(InvalidBlessingError::new(
                "stored blessing",
                key,
                "record with an anchor matching its storage key",
            )
            .into());
        }
        Ok((record, scope))
    })
    .await?;
    fetched.sort_by(|left, right| left.0.cmp(&right.0));
    let selected = blessing_scope(query);
    Ok(fetched
        .into_iter()
        .filter_map(|(key, candidate, (record, scope))| {
            selected.intersects(&scope).then_some(StoredBlessing {
                key,
                commit: candidate.commit,
                scope,
                record,
            })
        })
        .collect())
}

/// Rejects partial revocation before any deletion can affect unrelated partitions.
pub(crate) fn require_contained_scope(
    selected: &BlessingScope,
    scope: &BlessingScope,
    key: &str,
) -> Result<(), AnalyzeError> {
    if !selected.contains(scope) {
        return Err(BlessingScopeConflictError::new(key, scope.to_string()).into());
    }
    Ok(())
}
