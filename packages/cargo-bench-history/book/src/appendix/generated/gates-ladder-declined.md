**Declined** by `residual_noise`. The detector computed 15.97 ns, against a demand of 17.38 ns. The gates below it never ran.

| Gate | Demand | Computed | Verdict |
|---|---|---|---|
| `split_located` | must hold | held | pass |
| `min_regime` | 5 points | 10 points | pass |
| `non_zero_delta` | above zero | 15.97 ns | pass |
| `relative_floor` | 3.0% | 15.7% | pass |
| `absolute_floor` | 1 ns | 15.97 ns | pass |
| `residual_noise` | 17.38 ns | 15.97 ns | **declined** |
| `regime_separation` | 0.85 | not run | not run |
| `interval_disjoint` | the two intervals must not overlap | not run | not run |
| `selection_adjustment` | corrects the level; never declines | not run | not run |
| `significance` | p < 0.05 | not run | not run |
