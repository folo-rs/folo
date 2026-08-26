| Reason | What it means | What to do |
|---|---|---|
| `ghost` | A series not measured at the analyzed context commit. | Nothing, if the benchmark was removed or its package was not built. Otherwise check that it still runs at the analyzed context commit. |
| `too_few_points` | A series with too few points in the analyzed window. | Wait. The series is judged once enough commits have been measured. |
| `too_few_points_since_blessing` | A series with too few points since being blessed. | Wait. A blessing discards the evidence before it, so the count restarts. |
| `not_measured_on_branch` | A series not measured on the branch. | Run the benchmark on the branch, or accept that this one is out of scope for the comparison. |
| `too_few_base_commits` | A series with too few base-ref commits to compare against. | Measure more of the base ref. A comparison needs a base window to compare against. |
| `too_few_base_commits_since_blessing` | A series with too few base-ref commits remaining since being blessed. | Wait for more base-ref measurements after the blessing. Earlier evidence was intentionally excluded. |
| `current_base_regime_unresolved` | A series whose current base regime is unresolved. | Inspect the recent base history. It appears to have moved, but the new level has too little support for an honest comparison. |
