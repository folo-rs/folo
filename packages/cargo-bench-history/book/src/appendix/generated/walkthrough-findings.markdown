# Benchmark history analysis: textproc

2 regressions detected.

> [!WARNING]
> Incomplete coverage. Unjudged series may contain changes.

Judged 5 of 8 in-scope series.

Not judged (including out-of-scope series): 2 series not measured at the analyzed context commit; 1 series ignored by configuration; 3 series with too few points in the analyzed window.

- Commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
- Mode: history
- Runs analyzed: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)

## criterion / x86\_64-unknown-linux-gnu / machine a1b2c3d4

- Runs: 128
- Regressions: 2

### `http_parse/case`
**regression: +29.57%** wall time

regression via change point · before representative: 100.4 ns → after representative: 130.1 ns · change: +29.68 ns · somewhere near commit10

```text
 135 ┤           ╭─╮╭─╮╭╮           
 126 ┤         ╭─╯ ╰╯ ╰╯╰─          
 116 ┤         │                    
 106 ┤ ╭╮ ╭─╮╭─╯                    
  97 ┼─╯╰─╯ ╰╯                     
```

### `index_build/case`
**regression: +10.40%** wall time

regression via drift · before representative: 98.5 ns → after representative: 108.7 ns · change: +10.24 ns · accumulated commit0 → commit29

```text
 113 ┤                    ╭╮    ╭── 
 109 ┤          ╭╮╭╮╭─╮   ││ ╭╮╭╯   
 105 ┤ ╭╮     ╭─╯╰╯╰╯ ╰─╮╭╯╰─╯╰╯    
 101 ┼╮│╰─────╯         ╰╯          
  97 ┤╰╯                           
```

## Coverage

- State: `partial`
- Metric series accounted for: 11
- In scope: 8
- Judged: 5
- Unjudged (including out-of-scope series): 6

| Unjudged reason | Metric series |
| --- | --- |
| not measured at the analyzed context commit | 2 |
| ignored by configuration | 1 |
| with too few points in the analyzed window | 3 |
