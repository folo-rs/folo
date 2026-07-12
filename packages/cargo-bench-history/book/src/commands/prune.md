# prune

`prune` deletes a chosen portion of the stored data set — to reclaim storage, discard a bad
run, or drop the ephemeral uncommitted-tree snapshots that evaluation runs leave behind. It
reuses [`analyze`](analyze.md)'s selection pipeline and then removes the selected objects
rather than reporting on them.

```console
# Preview what would be deleted without deleting anything.
cargo bench-history prune --local=./bench-history --dry-run <scope>
```

## Deletion scope is required

A deletion scope is required — clean runs (and the blessings riding on them), dirty snapshots
only, or both — so a bare `prune` is an error that names the three.

## What prune preserves

Pruning never touches base-branch history: it walks the selected commits from the context back
to the merge-base with the base and deletes only the context branch's own commits, preserving
the shared base. Deleting the base branch's own data set wipes the mainline every feature
analysis compares against, so it is refused unless a confirming flag is passed.

A blessing is removed only when the clean run it annotates is removed in the same pass, so
blessings follow their clean run and are never time-filtered directly. A dry run builds the
identical plan but skips the deletes.
