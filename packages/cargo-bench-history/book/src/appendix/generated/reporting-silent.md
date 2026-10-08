**Full Markdown report** — written by `--markdown`.

````markdown
# Benchmark history analysis: textproc

- Commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
- Mode: history
- Runs analyzed: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)
- In-scope series judged: 3 of 6
- Regressions: 0

No notable changes detected among the series that were judged.

Judged 3 of 6 in-scope series; no reportable move survived the gates.

Not judged: 2 series not measured at the analyzed context commit; 3 series with too few points in the analyzed window.

## Coverage

- State: `partial`
- Metric series accounted for: 8
- In scope: 6
- Judged: 3
- Unjudged (including out-of-scope series): 5

| Unjudged reason | Metric series |
| --- | --- |
| not measured at the analyzed context commit | 2 |
| with too few points in the analyzed window | 3 |
````

**JSON report** — the same analysis from `--json`, with counts under `census`.

```json
{
  "project": "textproc",
  "tip_commit": "9f2c4a1d3b5e708aab12cd34ef5678901234567a",
  "tip_dirty": false,
  "mode": "history",
  "outcome": "partial",
  "notable": false,
  "runs": 128,
  "series": 6,
  "regressions": 0,
  "ghosts_excluded": 2,
  "census": {
    "total": 8,
    "in_scope": 6,
    "judged": 3,
    "unjudged": 5,
    "coverage": "partial",
    "reasons": [
      {
        "reason": "ghost",
        "count": 2
      },
      {
        "reason": "too_few_points",
        "count": 3
      }
    ]
  },
  "findings": [],
  "sets": [
    {
      "engine": "criterion",
      "target_triple": "x86_64-unknown-linux-gnu",
      "machine_key": "a1b2c3d4",
      "runs": 128,
      "series": 6,
      "regressions": 0
    }
  ]
}
```
