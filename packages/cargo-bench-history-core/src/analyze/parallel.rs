//! Small data-parallel helpers built on scoped threads.
//!
//! Several analysis passes walk a slice whose elements are independent — detecting
//! each series, sorting each series' points, parsing each fetched object — and they
//! share one shape: split the slice into one balanced, contiguous chunk per worker
//! thread, process each chunk on a scoped thread, and recombine in input order. That
//! shape lives here once rather than being re-implemented per call site.
//!
//! A single available CPU (Miri reports one by default) or a single-element slice
//! needs no parallelism and takes a plain serial pass — which is also the path Miri
//! exercises, so the callers' logic stays under Miri's checks without spawning
//! threads Miri does not model.

use std::num::NonZero;
use std::{panic, thread};

/// How many worker threads to split `len` items across: the available parallelism,
/// capped at `len` so every spawned worker gets at least one item (never an empty
/// chunk).
//
// Mutation-skipped: the return value only selects how the work is partitioned, never
// the result. Both helpers take a serial pass for `<= 1`, and every worker count
// yields the same order-preserving output, so no behavioral test can distinguish the
// values.
#[cfg_attr(test, mutants::skip)]
fn worker_count(len: usize) -> usize {
    thread::available_parallelism()
        .map_or(1, NonZero::get)
        .min(len)
}

/// The lengths of exactly `workers` contiguous chunks that partition `len` items as
/// evenly as possible: the first `len % workers` chunks hold one extra item, the
/// rest hold `len / workers`.
///
/// Both callers guarantee `1 <= workers <= len` before taking the parallel path (a
/// single worker, or a slice no longer than the worker count, takes the serial path
/// instead), so every chunk is non-empty and the sizes differ by at most one.
fn balanced_chunk_sizes(len: usize, workers: usize) -> impl Iterator<Item = usize> {
    // `workers >= 1` here, so the divide and remainder are well defined, and
    // `base + 1 <= len` cannot overflow; the checked operators keep the workspace's
    // arithmetic lint satisfied without masking a real bug.
    let base = len.checked_div(workers).unwrap_or(0);
    let remainder = len.checked_rem(workers).unwrap_or(0);
    (0..workers).map(move |index| {
        if index < remainder {
            base.saturating_add(1)
        } else {
            base
        }
    })
}

/// Maps `worker` over `items` in parallel, returning the results in input order.
///
/// The slice is divided into exactly one contiguous chunk per worker thread —
/// balanced, so the chunk sizes differ by at most one and every worker gets work;
/// each chunk is mapped on its own scoped thread and the chunk results are
/// concatenated in order, so the output is identical to
/// `items.iter().map(worker).collect()` — only spread across cores. A worker panic
/// is propagated to the caller.
///
/// `worker` must be `Sync` because every chunk shares one reference to it.
pub fn map_parallel<T, R, F>(items: &[T], worker: F) -> Vec<R>
where
    T: Sync,
    R: Send,
    F: Fn(&T) -> R + Sync,
{
    let workers = worker_count(items.len());
    if workers <= 1 {
        return items.iter().map(&worker).collect();
    }

    let worker = &worker;
    let total = items.len();
    thread::scope(|scope| {
        // Hand each worker one balanced subslice carved off the front with
        // `split_at`, so every chunk is an owned subslice with the slice's full
        // lifetime that moves cleanly into its thread. Spawning every worker up front
        // (before any `join`) is what runs them concurrently; joining one chunk at a
        // time would serialize the work.
        let mut remaining = items;
        let mut handles = Vec::with_capacity(workers);
        for size in balanced_chunk_sizes(total, workers) {
            let (chunk, rest) = remaining.split_at(size);
            remaining = rest;
            handles.push(scope.spawn(move || chunk.iter().map(worker).collect::<Vec<R>>()));
        }
        debug_assert!(
            remaining.is_empty(),
            "balanced sizes sum to the slice length"
        );
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|payload| panic::resume_unwind(payload))
            })
            .collect()
    })
}

/// Applies `worker` to every element of `items` in place, in parallel.
///
/// The mutable counterpart of [`map_parallel`] for an in-place pass with nothing to
/// gather (each element is transformed where it sits). The slice is split into
/// exactly one balanced contiguous chunk per worker via repeated
/// [`split_at_mut`](slice::split_at_mut), so the chunks are provably disjoint and
/// each worker owns its elements. A worker panic is propagated to the caller.
pub(crate) fn for_each_mut_parallel<T, F>(items: &mut [T], worker: F)
where
    T: Send,
    F: Fn(&mut T) + Sync,
{
    let workers = worker_count(items.len());
    if workers <= 1 {
        items.iter_mut().for_each(&worker);
        return;
    }

    let worker = &worker;
    let total = items.len();
    thread::scope(|scope| {
        // Carve one balanced subslice per worker off the front with `split_at_mut`;
        // the pieces are provably disjoint, so each moves into its own thread. Spawn
        // them all before joining so they run concurrently.
        let mut remaining = items;
        let mut handles = Vec::with_capacity(workers);
        for size in balanced_chunk_sizes(total, workers) {
            let (chunk, rest) = remaining.split_at_mut(size);
            remaining = rest;
            handles.push(scope.spawn(move || chunk.iter_mut().for_each(worker)));
        }
        debug_assert!(
            remaining.is_empty(),
            "balanced sizes sum to the slice length"
        );
        for handle in handles {
            handle
                .join()
                .unwrap_or_else(|payload| panic::resume_unwind(payload));
        }
    });
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn balanced_chunk_sizes_splits_into_exactly_workers_balanced_chunks() {
        // An even split gives every worker the same size.
        let even: Vec<usize> = balanced_chunk_sizes(12, 4).collect();
        assert_eq!(even, vec![3, 3, 3, 3]);

        // A remainder is spread one-per-chunk across the leading chunks, so the
        // larger chunks come first and sizes differ by at most one.
        let uneven: Vec<usize> = balanced_chunk_sizes(13, 4).collect();
        assert_eq!(uneven, vec![4, 3, 3, 3]);

        // The "just above the worker count" case the old `div_ceil` chunking got
        // wrong (len = workers + 1 yielded ~workers/2 chunks): it must still produce
        // exactly `workers` non-empty chunks — one of size two, the rest size one —
        // never collapse to fewer and leave cores idle.
        let just_above: Vec<usize> = balanced_chunk_sizes(33, 32).collect();
        assert_eq!(just_above.len(), 32);
        assert_eq!(just_above.iter().filter(|&&size| size == 2).count(), 1);
        assert_eq!(just_above.iter().filter(|&&size| size == 1).count(), 31);

        // Across many shapes: exactly `workers` chunks, every chunk non-empty, sizes
        // differ by at most one, and they sum back to `len` (a full, non-overlapping
        // partition).
        for len in 1..=64_usize {
            for workers in 1..=len.min(16) {
                let sizes: Vec<usize> = balanced_chunk_sizes(len, workers).collect();
                assert_eq!(sizes.len(), workers, "len={len} workers={workers}");
                assert_eq!(
                    sizes.iter().sum::<usize>(),
                    len,
                    "len={len} workers={workers}"
                );
                let max = *sizes.iter().max().expect("workers >= 1");
                let min = *sizes.iter().min().expect("workers >= 1");
                assert!(min >= 1, "len={len} workers={workers}: {sizes:?}");
                assert!(
                    max.saturating_sub(min) <= 1,
                    "len={len} workers={workers}: {sizes:?}"
                );
            }
        }
    }

    #[test]
    fn map_parallel_preserves_order_and_maps_every_element() {
        let input: Vec<usize> = (0..1000).collect();
        let output = map_parallel(&input, |value| value.saturating_mul(2));
        let expected: Vec<usize> = (0..1000).map(|value| value * 2).collect();
        assert_eq!(output, expected);
    }

    #[test]
    fn map_parallel_handles_empty_and_single_element_slices() {
        let empty: Vec<usize> = Vec::new();
        assert!(map_parallel(&empty, |value| *value).is_empty());

        let single = [7_usize];
        assert_eq!(
            map_parallel(&single, |value| value.saturating_add(1)),
            vec![8]
        );
    }

    #[test]
    fn for_each_mut_parallel_mutates_every_element_in_place() {
        let mut values: Vec<usize> = (0..1000).collect();
        for_each_mut_parallel(&mut values, |value| *value = value.saturating_add(100));
        let expected: Vec<usize> = (0..1000).map(|value| value + 100).collect();
        assert_eq!(values, expected);
    }

    #[test]
    fn for_each_mut_parallel_handles_empty_and_single_element_slices() {
        let mut empty: Vec<usize> = Vec::new();
        for_each_mut_parallel(&mut empty, |value| *value = value.saturating_add(1));
        assert!(empty.is_empty());

        let mut single = [7_usize];
        for_each_mut_parallel(&mut single, |value| *value = value.saturating_add(1));
        assert_eq!(single, [8]);
    }

    #[test]
    #[should_panic(expected = "worker boom")]
    fn map_parallel_propagates_a_worker_panic() {
        // A panic in any chunk surfaces to the caller rather than being swallowed.
        // The slice is large enough that the parallel path is taken when more than
        // one core is available; the serial path panics just the same.
        let input: Vec<usize> = (0..1000).collect();
        let _ = map_parallel(&input, |value| {
            assert_ne!(*value, 500, "worker boom");
            *value
        });
    }
}
