Each detector applies its own gates in its own order, and a candidate stops at the first gate that declines it. A gate several detectors share is one policy asked at a different point in each sequence.

**`change_point`** — a level that moved and stayed moved

| Gate | What it compares | Threshold |
|---|---|---|
| `split_located` | Whether a candidate split exists at all. | a split must be found |
| `min_regime` | How many points the shorter side of the split holds. | 5 points |
| `non_zero_delta` | Whether the two levels differ at all. | above zero |
| `significance` | The chance level of the rank test comparing the two regimes. | p < 0.05 |
| `relative_floor` | The move as a fraction of the baseline. | 3.0% |
| `absolute_floor` | The move in the metric's own units. | the metric's own floor, below |
| `residual_noise` | The move against the series' own typical residual. | 3× the typical residual |
| `regime_separation` | The share of before-and-after pairs that agree the level moved. | 0.85 |
| `interval_disjoint` | The two regimes' reported confidence intervals. | the two intervals must not overlap |

**`drift`** — a level that is moving steadily

| Gate | What it compares | Threshold |
|---|---|---|
| `min_series_points` | How many points the analyzed window holds. | 10 points |
| `significance` | The chance level of the trend test across the window. | p < 0.05 |
| `non_zero_delta` | Whether the two levels differ at all. | above zero |
| `relative_floor` | The move as a fraction of the baseline. | 3.0% |
| `absolute_floor` | The move in the metric's own units. | the metric's own floor, below |
| `residual_noise` | The move against the series' own typical residual. | 3× the typical residual |
| `interval_noise_band` | The move against the engine's own reported imprecision. | 2× the reported half-width |

**`branch`** — a branch tip against the base it forked from

| Gate | What it compares | Threshold |
|---|---|---|
| `min_base_commits` | How many base-side commit levels the comparison window holds. | 10 commit levels |
| `non_zero_delta` | Whether the two levels differ at all. | above zero |
| `min_regime` | How many points the shorter side of the split holds. | 5 points |
| `relative_floor` | The move as a fraction of the baseline. | 5.0% |
| `absolute_floor` | The move in the metric's own units. | the metric's own floor, below |
| `residual_noise` | The move against the series' own typical residual. | 3× the typical residual |
| `base_scatter` | Whether the base window carries enough scatter to form an interval. | the sample must carry some scatter |
| `significance` | The chance level of the tip against the base window's interval. | p < 0.05 |
| `interval_disjoint` | The two regimes' reported confidence intervals. | the two intervals must not overlap |
| `interval_noise_band` | The move against the engine's own reported imprecision. | 2× the reported half-width |
