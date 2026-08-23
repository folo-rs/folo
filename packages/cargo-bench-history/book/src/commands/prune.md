# prune

`prune` deletes a chosen portion of the stored data set — to reclaim storage, discard a bad
run, or drop the ephemeral uncommitted-tree snapshots that evaluation runs leave behind. It
reuses [`analyze`](analyze.md)'s selection pipeline and then removes the selected objects
rather than reporting on them.

```console
# Preview deletion of dirty snapshots without deleting anything.
cargo bench-history prune --local=./bench-history --dry-run --dirty

# Delete clean runs from the selected feature-branch commits.
cargo bench-history prune --local=./bench-history --clean

# Also delete the blessing sidecars in the selected range.
cargo bench-history prune --local=./bench-history --clean --include-blessings
```

## An action is required

An action is required: `--clean` removes clean runs, `--dirty` removes only dirty snapshots,
and `--all` removes both. `--include-blessings` additionally deletes blessing sidecars and may
be given on its own to remove only blessings. A bare `prune` is therefore an error that names
these choices.

## What prune preserves

By default, pruning walks from the context back to its merge-base and deletes only the feature
branch's own commits, preserving shared base-branch history. If the context is itself on the
base branch, deletion is refused unless you explicitly confirm it with `--prune-base`; that
guard protects the mainline data every feature analysis compares against.

Pruning runs never removes a blessing; only `--include-blessings` (or [`unbless`](bless.md))
does. With `--include-blessings`, every blessing in the selected range is deleted — including
an orphan on a commit that has no recorded run. A dry run builds the identical plan but skips
the deletes.
