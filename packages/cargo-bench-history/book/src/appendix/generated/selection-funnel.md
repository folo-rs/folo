| Stage | What it removes from this store | Objects removed | Still eligible |
|---|---|--:|--:|
| Every run object in this store | | | 54 |
| Facet filter | every object of `criterion / macos-arm64 / 7c6d`, a partition the query's facets do not name | 20 | 34 |
| On the analyzed history | runs recorded at commits that are not on the context's first-parent line | 3 | 31 |
| Dirty admission | 1 dirty run, recorded at or before the merge base (commit 19) | 1 | 30 |
| `--since` | everything still eligible that is older than commit 6 | 8 | 22 |
| Fetched and parsed | | | 22 |

32 runs removed and 22 runs account for all 54 run objects this store held. Only those survivors are fetched and parsed; every other run was decided on its storage key and the commit's place in the topology alone.

This worked store holds only runs. A matching blessing sidecar is a separate object kind: it is set apart during facet selection and follows its own path, so it never enters the run-only topology, dirty-admission, and window stages counted here.

The grid draws the analyzed first-parent line, so the runs the on-history stage removed have no column in it — having no place on that line is exactly why they were removed. Partitions are labelled by engine, target triple, and machine key, with the triples shortened to fit.
