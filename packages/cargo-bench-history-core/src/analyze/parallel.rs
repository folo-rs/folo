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

/// Maps `worker` over `items` in parallel, returning the results in input order.
///
/// The slice is divided into up to one contiguous chunk per worker thread (equal
/// sized except for a possibly shorter last chunk); each chunk is mapped on its own
/// scoped thread and the chunk results are concatenated in order, so the output is
/// identical to `items.iter().map(worker).collect()` — only spread across cores. A
/// worker panic is propagated to the caller.
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
    let chunk_size = items.len().div_ceil(workers);
    thread::scope(|scope| {
        // Each `chunks` element is an owned subslice with the slice's full lifetime,
        // so it moves cleanly into its worker thread. The eager `collect` spawns
        // every worker before the first `join`; a lazy `flat_map` straight onto the
        // handles would instead spawn and join one chunk at a time, serializing the
        // work.
        let handles: Vec<_> = items
            .chunks(chunk_size)
            .map(|chunk| scope.spawn(move || chunk.iter().map(worker).collect::<Vec<R>>()))
            .collect();
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
/// gather (each element is transformed where it sits). The slice is split into up to
/// one contiguous chunk per worker via [`chunks_mut`](slice::chunks_mut), so the
/// chunks are provably disjoint and each worker owns its elements. A worker panic is
/// propagated to the caller.
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
    let chunk_size = items.len().div_ceil(workers);
    thread::scope(|scope| {
        let handles: Vec<_> = items
            .chunks_mut(chunk_size)
            .map(|chunk| scope.spawn(move || chunk.iter_mut().for_each(worker)))
            .collect();
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
