**JSON** — the same analysis, written by `--json`.

```json
{
  "project": "textproc",
  "tip_commit": "9f2c4a1d3b5e708aab12cd34ef5678901234567a",
  "tip_dirty": false,
  "mode": "history",
  "notable": true,
  "runs": 128,
  "series": 51,
  "regressions": 2,
  "improvements": 0,
  "ghosts_excluded": 3,
  "census": {
    "total": 54,
    "in_scope": 51,
    "judged": 46,
    "unjudged": 8,
    "coverage": "partial",
    "reasons": [
      {
        "reason": "ghost",
        "count": 3
      },
      {
        "reason": "too_few_points",
        "count": 5
      }
    ]
  },
  "findings": [
    {
      "engine": "criterion",
      "target_triple": "x86_64-unknown-linux-gnu",
      "machine_key": "a1b2c3d4",
      "segments": [
        "http_parse",
        "case"
      ],
      "kind": "wall_time",
      "method": "change_point",
      "direction": "regression",
      "baseline": 100.36830218253652,
      "latest": 130.05171405014943,
      "relative_delta": 0.29574488381430086,
      "confidence": 0.9998173282088905,
      "commit": "commit10"
    },
    {
      "engine": "criterion",
      "target_triple": "x86_64-unknown-linux-gnu",
      "machine_key": "a1b2c3d4",
      "segments": [
        "index_build",
        "case"
      ],
      "kind": "wall_time",
      "method": "drift",
      "direction": "regression",
      "baseline": 98.50313347697937,
      "latest": 108.7433855201543,
      "relative_delta": 0.10395864254986495,
      "confidence": 0.9999958352376284,
      "commit": "commit29"
    }
  ],
  "sets": [
    {
      "engine": "criterion",
      "target_triple": "x86_64-unknown-linux-gnu",
      "machine_key": "a1b2c3d4",
      "runs": 128,
      "series": 51,
      "regressions": 2,
      "improvements": 0
    }
  ]
}
```
