A finding, as the text report prints it:

```text
http_parse/case
  +29.57% wall_time
    regression via change point · 100.4 → 130.1 · @ commit10
 135 ┤           ╭─╮╭─╮╭╮           
 126 ┤         ╭─╯ ╰╯ ╰╯╰─          
 116 ┤         │                    
 106 ┤ ╭╮ ╭─╮╭─╯                    
  97 ┼─╯╰─╯ ╰╯
```

| Part | What it carries |
|---|---|
| `http_parse/case` | The benchmark identity: every benchmark-ID segment joined by `/`. This slash-rendered identity is the string accepted by [`examine`](../commands/examine.md) and matched by the benchmark-ID prefix supplied to [`bless`](../commands/bless.md). |
| `+29.57% wall_time` | The headline: the move as a percentage of the baseline, and the metric kind that moved. Findings are ranked by the magnitude of that percentage. |
| `regression via change point · 100.4 → 130.1 · @ commit10` | The detail: direction, the detector that produced the finding, the baseline and latest representative values, and the commit the change is attributed to. |
| the lines below it | The chart: the series drawn against topology, one column per commit. |
