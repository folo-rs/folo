| Policy | Behavior | When to use it |
|---|---|---|
| default | Refuses, leaving the stored run untouched | Always, unless you have a reason not to |
| `--skip-existing` | Leaves the stored run and reports success | Re-running a collection over a range where some commits are already done |
| `--overwrite` | Replaces the stored run | Re-measuring a commit whose recorded run you do not trust |
