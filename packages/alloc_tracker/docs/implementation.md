# alloc_tracker implementation

User-visible behavior is defined in the package [design](design.md).

## Counters

The allocator wrapper maintains one counter block per thread: cumulative bytes, cumulative
allocation count, currently outstanding bytes, and a watermark of outstanding bytes. Blocks
are shared through a process-wide registry so that process-scope measurement can sum across
threads, and each thread caches a pointer to its own block in thread-local storage.

Only the owning thread ever writes to a block. That single-writer discipline is what makes
the hot path cheap: the counters are atomics purely so that other threads may read them, and
writes are a relaxed load followed by a relaxed store rather than a read-modify-write. Cross-
thread readers accordingly see values that may be slightly stale, which is acceptable because
process-scope figures are approximate by nature.

Outstanding bytes is signed. A block is attributed to whichever thread performs the
deallocation, so a thread that frees memory allocated elsewhere drives its own outstanding
count below zero. This is a deliberate consequence of measuring allocator events per thread
rather than tracking ownership of live allocations, which would require per-allocation
bookkeeping.

## Reentrancy

Tracking runs inside the global allocator, so it must not allocate. Registering a thread's
counter block does allocate — it constructs a shared block and inserts it into the registry —
so a guard flag marks the window during which a thread is initializing its own block, and
allocations occurring inside that window go untracked.

The thread-local pointer is published as the last step before the guard is cleared, and
nothing between those two steps allocates. The presence of the pointer therefore already
proves that the thread is outside the initialization window, so the hot path performs a
single thread-local lookup and needs no separate guard check.

Deallocation never initializes a counter block. A free from a thread that has none is simply
not counted. Initializing on the deallocation path would re-enter the allocator, and would
also consume the one-shot flag used by the panic-on-next-allocation debugging feature.

Recording happens after the inner allocator call and only when it succeeded, so a failed
allocation moves no counters.

## Watermark protocol

The watermark is per-thread state, but the metric it feeds is per-span, so spans hand the
watermark back and forth rather than owning it.

A thread span records the current outstanding level and the current watermark on entry, then
lowers the watermark to the entry level. From that point the watermark tracks only what this
span itself accumulates. On drop the span takes the difference between the watermark and its
entry level as its own peak, and restores the enclosing watermark to the higher of its saved
value and the level this span reached — memory held by an inner span was equally outstanding
from the enclosing span's perspective.

Restoration happens before every early exit in the drop path, including the panic-unwinding
case and the missing-iteration-count panic. A span abandoned without recording must still
return the watermark, or it would silently suppress the peak of the span enclosing it.

The hand-back is what makes reverse-order drops a requirement rather than a convention:
restoring an outer span's saved watermark while an inner span is still live would raise the
inner span's baseline and inflate its reported peak. The requirement is stated in the
design rather than enforced, because enforcing it would mean giving every span an identity
and a stack to check against, which is more machinery than an ordering convention that
scoped bindings satisfy automatically.

## Peak aggregation

An operation folds the peaks of its spans through a tri-state: no span recorded yet, a known
maximum, or unavailable. The distinction between the first and third state is what allows a
single process-scope span to render the whole operation's peak unavailable, rather than
having an unmeasurable span fold as a harmless zero. Merging two reports folds the two
tri-states the same way.

## Reporting

The human-readable table is rendered from a fixed column set with widths computed from the
formatted cell contents, so adding a column does not require touching the layout logic. The
JSON output omits the peak field entirely when it is unavailable, which keeps the field
additive for existing consumers.
