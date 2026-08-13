| Reason | The line the report prints | What it means |
|---|---|---|
| discriminant set mismatch | `Warning: comparison base is 7 commits behind base (discriminant set mismatch)` | Newer base data exists, but it was measured under a different machine key. Counts are never compared across machine keys, so the comparison reached back to the newest base commit this partition covers. A rotating CI pool is the usual cause. |
| no base data at more recent commits | `Warning: comparison base is 1 commit behind base (no base data at more recent commits)` | No base-side run for the series exists at any more recent commit. The comparison base is simply the newest base data there is, and collection on the base branch is what would move it forward. |

The lag is advisory. It never changes which findings are reported and never affects the exit code; what it changes is how much weight a marginal branch finding deserves, because a comparison against a base state several commits old is exactly that.
