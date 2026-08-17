| Engine | Confidence interval | Standard deviation |
|---|---|---|
| `callgrind` | Never — it reports a single simulated figure | Never |
| `criterion` | Always | Recorded, never read |
| `alloc_tracker` | Only when the operation was measured over several spans | Never |
| `all_the_time` | Only when the operation was measured over several spans | Never |
