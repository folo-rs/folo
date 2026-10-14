# Benchmark history analysis: textproc

1 regression, 0 improvements detected.

Judged 1 of 1 in-scope series (one benchmark/metric per platform/engine/machine).

- Commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
- Mode: branch
- Runs analyzed: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)
- In-scope series judged: 1 of 1
- Regressions: 1
- Improvements: 0

## criterion / x86\_64-unknown-linux-gnu / machine a1b2c3d4

- Runs: 128
- Regressions: 1
- Improvements: 0

**Historical comparison:** None of 5 comparable base commits showed as much out-of-range movement as this branch (1 series compared).

### `http_parse/case`
**regression: 126.1 ns** wall time - slower than all 10 current-base observations

current base range: 96.93 ns to 103.5 ns · excess beyond nearest range edge: +22.6 ns / +21.85% · @ 9f2c4a1d3b5e708aab12cd34ef5678901234567a

```text
wall time (ns, linear scale; lower is better)
 126 ┤          ╭ 
 119 ┤          │ 
 111 ┤          │ 
 104 ┼────╮╭────╯ 
  97 ┤    ╰╯     
Comparison: range edge | recent base history -> CONTEXT 9f2c4a1d3b5e (last column).
Context is placed after base, not a continuation of the branch's commit history.
Empty columns = missing base measurements; intermediate branch commits are omitted.
```

## Coverage

- State: `full`
- Metric series accounted for: 1
- In scope: 1
- Judged: 1
- Unjudged (including out-of-scope series): 0
