The funnel below traces one worked query — a host-local `analyze` — against this store. Two of its inputs drive the removals that would otherwise look arbitrary: the **discriminant filters** resolve to this host's own partitions, so a foreign partition measured on another machine is dropped; and **`--since`** resolves to a cutoff date part way through the history, dropping everything committed before it. Each row is one selection stage and what it removed.

| Stage | What it removes from this store | Objects removed | Still eligible |
|---|---|--:|--:|
| Every run object in this store | | | 54 |
| Discriminant filter | every object of `criterion / macos-arm64 / 7c6d`, a partition the query's discriminant filters do not name | 20 | 34 |
| On the analyzed history | runs recorded at commits that are not on the context's first-parent line | 3 | 31 |
| Dirty admission | 1 dirty run, recorded at or before the merge base (commit 19) | 1 | 30 |
| `--since` | everything still eligible that is older than commit 6 | 8 | 22 |
| Fetched and parsed | | | 22 |

32 runs removed and 22 runs account for all 54 run objects this store held. Only those survivors are fetched and parsed; every other run was decided on its storage key and the commit's place in the topology alone.

The store in this worked example holds only **runs** — stored benchmark measurements (see [Shape of the data](shape.md#what-a-stored-run-holds)). A **blessing**, a recorded acceptance of a change (see [Reconstruction](reconstruction.md#blessings)), is a separate object kind stored alongside them: it is set apart during discriminant selection and follows its own path, so it never enters the run-only topology, dirty-admission, and window stages counted here.

The grid draws the analyzed first-parent line, so the runs the on-history stage removed have no column in it — having no place on that line is exactly why they were removed. Partitions are labeled by engine, target triple, and machine key, with the triples shortened to fit.
