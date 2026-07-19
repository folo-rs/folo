# prune

`prune` deletes a chosen portion of the stored data set — to reclaim storage, discard a bad
run, or drop the ephemeral uncommitted-tree snapshots that evaluation runs leave behind. It
reuses [`analyze`](analyze.md)'s selection pipeline and then removes the selected objects
rather than reporting on them.

```console
# Preview deletion of dirty snapshots without deleting anything.
cargo bench-history prune --local=./bench-history --dry-run --dirty

# Delete clean runs (and their blessings) from the selected feature-branch commits.
cargo bench-history prune --local=./bench-history --clean
```

## Deletion scope is required

A deletion scope is required: `--clean` removes clean runs and their blessings, `--dirty`
removes only dirty snapshots, and `--all` removes both. A bare `prune` is therefore an
error that names the three choices.

## What prune preserves

By default, pruning walks from the context back to its merge-base and deletes only the feature
branch's own commits, preserving shared base-branch history. If the context is itself on the
base branch, deletion is refused unless you explicitly confirm it with `--prune-base`; that
guard protects the mainline data every feature analysis compares against.

A blessing is removed only when the clean run it annotates is removed in the same pass, so
blessings follow their clean run and are never time-filtered directly. A dry run builds the
identical plan but skips the deletes.
