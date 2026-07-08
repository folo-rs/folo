use std::path::{Component, Path};

use cbh_model::{OBJECTS_SEGMENT, STORAGE_VERSION, sanitize_segment};

use super::StorageError;

/// Returns `true` only if `segment` is a single ordinary path component, so it
/// can never escape or rebind a storage root when appended to a key.
pub(crate) fn is_plain_segment(segment: &str) -> bool {
    let mut components = Path::new(segment).components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

/// Validates that `key` is a well-formed object key: a `/`-separated sequence of
/// ordinary path segments. Rejects empty, `.`, `..`, and platform-absolute
/// segments, which could otherwise escape or rebind a filesystem storage root.
///
/// Both backends share this so the in-memory fake rejects exactly the keys the
/// real [`LocalStorage`](super::local::LocalStorage) would, keeping fake-driven tests
/// honest.
///
/// # Errors
///
/// Returns [`StorageError::InvalidKey`] if any segment of `key` is not a single
/// ordinary path component.
pub(crate) fn validate_key(key: &str) -> Result<(), StorageError> {
    for segment in key.split('/') {
        if !is_plain_segment(segment) {
            return Err(StorageError::InvalidKey {
                key: key.to_owned(),
            });
        }
    }
    Ok(())
}

/// The reserved file-name segment of the per-project cache-invalidation marker.
///
/// It is a **metadata sibling** of the project's data subtree: data objects live
/// under `{STORAGE_VERSION}/<project>/{OBJECTS_SEGMENT}/…`, while the marker sits
/// directly under `{STORAGE_VERSION}/<project>/`, so it can never appear in a data
/// listing (which narrows to the `objects/` prefix) and can never be mistaken for a
/// data key. The leading `_` keeps it visually distinct from future metadata kinds.
const CACHE_EPOCH_SEGMENT: &str = "_cache-epoch";

/// The storage key of a project's cache-invalidation marker:
/// `{STORAGE_VERSION}/<project>/_cache-epoch`.
///
/// A cloud writer overwrites it with a fresh opaque epoch token whenever it
/// removes or rewrites an existing object (`delete`/`put_overwrite`); the
/// read-through cache compares its cloud value against the copy its mirror recorded
/// under the *same key* to decide whether to reuse or wipe that project's mirrored
/// objects. The `project` segment is sanitized identically to
/// `DiscriminantSet::partition_prefix`
/// so the marker lands in the same partition as that project's data.
pub(crate) fn cache_epoch_key(project: &str) -> String {
    format!(
        "{STORAGE_VERSION}/{project}/{CACHE_EPOCH_SEGMENT}",
        project = sanitize_segment(project)
    )
}

/// The listing prefix that covers every **data** object of `project`:
/// `{STORAGE_VERSION}/<project>/{OBJECTS_SEGMENT}/`.
///
/// Data loaders (`analyze`/`list`/`prune`, `backfill`'s recorded-commit scan) list
/// under this prefix so they enumerate only benchmark objects, never a per-project
/// metadata sibling such as the [`cache_epoch_key`] marker. The read-through cache
/// also wipes exactly this subtree when invalidating one project's mirror.
#[must_use]
pub fn project_objects_prefix(project: &str) -> String {
    format!(
        "{STORAGE_VERSION}/{project}/{OBJECTS_SEGMENT}/",
        project = sanitize_segment(project)
    )
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn cache_epoch_key_lands_in_the_project_partition() {
        // The marker is a metadata sibling of the project's objects/ subtree: it sits
        // directly under v1/<project>/, so it never appears in a data listing (which
        // narrows to the objects/ prefix) and cannot collide with a data object.
        assert_eq!(cache_epoch_key("folo"), "v1/folo/_cache-epoch");
        // The project is sanitized identically to the data partition, so the
        // marker always lands beside that project's objects.
        assert_eq!(cache_epoch_key("a b"), "v1/a_b/_cache-epoch");
    }

    #[test]
    fn project_objects_prefix_narrows_to_the_data_subtree() {
        // Data loaders list this prefix so they enumerate only benchmark objects and
        // never the sibling cache-epoch marker; the project is sanitized identically.
        assert_eq!(project_objects_prefix("folo"), "v1/folo/objects/");
        assert_eq!(project_objects_prefix("a b"), "v1/a_b/objects/");
        // The marker sits directly under the project partition, outside this prefix,
        // so a data listing can never pick it up.
        assert!(!cache_epoch_key("folo").starts_with(&project_objects_prefix("folo")));
    }
}
