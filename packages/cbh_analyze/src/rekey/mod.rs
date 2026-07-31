//! The `rekey` command: re-partition stored objects under the current machine-key
//! format.
//!
//! The machine key a run is stored under is a versioned hash of the host's hardware
//! factors. When the factor set changes, the same machine hashes to a new key and its
//! series is split in two: the history before the change sits under the old key and the
//! history after it under the new one, so neither is long enough to judge. `rekey`
//! repairs that split without re-running a single benchmark, because every stored run
//! embeds the [`MachineInfo`](cbh_model::MachineInfo) both key versions hash.
//!
//! It copies each eligible object to its current-version key and never deletes or
//! overwrites anything, so the old partition survives the migration untouched and a
//! second run is a no-op. Writing is opt-in: the default pass reports what it would do.
//!
//! `rekey` deliberately takes no discriminant facets. The sibling `analyze`/`list`/
//! `prune`/`examine` commands select a *facet* of the store to reason about; a
//! migration must instead see the whole store, since a partition it skipped would stay
//! fragmented and an operator would have no way to tell which ones were covered.
//!
//! # Runs whose provenance cannot be proven
//!
//! Host hardware (`context.machine`) is only recorded from schema version 4 onwards, so
//! a run written before it carries nothing to recompute a machine key from and cannot
//! be migrated at all. Such runs are counted and listed under `missing_provenance` in
//! the report and stay on the key they were written under, where they remain readable
//! but keep whatever fragmentation they already had.
//!
//! A run may also record hardware that the retired key format cannot render — the
//! facts arrive deserialized from an arbitrarily old object, so nothing guarantees they
//! describe a machine that could exist. No retired key can be recomputed for such a
//! run, so nothing proves its key segment was an auto-detected hash rather than an
//! operator's own, and moving it could file a damaged record under a healthy machine's
//! partition. These runs are counted and listed under `unrenderable_provenance` and
//! likewise stay where they are. A segment that already *is* the current hash is the
//! exception: the current format reads none of the factors the retired one dropped, so
//! such a run is placed without any retired hash and keeps its ordinary standing,
//! including its weight in the merge assessment.
//!
//! The dry run reports both, so an operator should read that section before applying
//! and decide whether the remaining history is long enough to judge on its own.
//!
//! # Recovering from an aborted apply
//!
//! Applying a plan copies objects one at a time. A destination that already holds
//! *different* bytes than the source would write is a conflict the migration refuses to
//! resolve, and it aborts the pass at that object rather than overwriting a distinct
//! measurement. Every copy made before that point stands, so the destination partition
//! is left **truncated**, not empty.
//!
//! A truncated partition still reads, which is what makes the symptom indirect: if it
//! now holds fewer commits than `min_series_points`, branch mode reports
//! `TooFewBaseCommits` — silence — instead of an error, and a regression on that
//! machine would go unjudged. Because the source partition is never deleted, resolving
//! the conflicting destination object and re-running the migration completes the copy
//! and restores the partition.

mod command;
mod legacy_key;
mod merge;
// Production data captured from the live benchmark store, consumed by the unit tests of
// the modules above as regression witnesses.
#[cfg(test)]
mod production_hardware;
#[cfg(test)]
mod production_merges;

pub use command::execute;
pub(crate) use legacy_key::legacy_machine_key;
pub(crate) use merge::{
    GroupPair, MeasuredPoint, MergeAnalysis, PartitionMerge, analyze_merges, merge_offset_tolerance,
};
