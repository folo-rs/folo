A finding, as the text report prints it:

```text
http_parse/case
  regression: +29.57% wall time
    regression via change point · before representative: 100.4 ns → after representative: 130.1 ns · change: +29.68 ns · somewhere near commit10
 135 ┤           ╭─╮╭─╮╭╮           
 126 ┤         ╭─╯ ╰╯ ╰╯╰─          
 116 ┤         │                    
 106 ┤ ╭╮ ╭─╮╭─╯                    
  97 ┼─╯╰─╯ ╰╯
```

| Part | What it carries |
|---|---|
| `http_parse/case` | The benchmark identity: every benchmark-ID segment joined by `/`. This slash-rendered identity is the string accepted by [`examine`](../commands/examine.md) and matched by the benchmark-ID prefix supplied to [`bless`](../commands/bless.md). |
| `regression: +29.57% wall time` | The headline: the move as a percentage of the baseline, and the metric kind that moved. Findings are ranked by the magnitude of that percentage. |
| `regression via change point · before representative: 100.4 ns → after representative: 130.1 ns · change: +29.68 ns · somewhere near commit10` | The detail: direction, the detector that produced the finding, the before and after representative values with units, and a commit somewhere near the change. |
| the lines below it | The chart: the series drawn against topology, one column per commit; its values use the units of the finding above it. |
