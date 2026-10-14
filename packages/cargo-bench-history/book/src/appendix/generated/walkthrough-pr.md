## Benchmark history: textproc

1 regression, 0 improvements detected.
Judged 1 of 1 in-scope series (one benchmark/metric per platform/engine/machine).

Context: 9f2c4a1d3b5e708aab12cd34ef5678901234567a · branch mode · 128 runs.
Change = excess beyond the nearest observed current-base range edge, not a mean-to-context change.
Historical comparison unavailable for 0 platform/engine/machine groups; 0 excursions outside the shared-history comparison.

<details>
<summary>Comparison evidence by platform</summary>

**criterion / x86\_64-unknown-linux-gnu / machine a1b2c3d4:** None of 5 comparable base commits showed as much out-of-range movement as this branch (1 series compared).


</details>


| Direction | Benchmark / metric | Platform / engine / machine | Current base range | Context measured | Excess beyond range edge |
| --- | --- | --- | --- | --- | --- |
| regression | http\_parse/case / wall time | Linux (x86\_64-unknown-linux-gnu) / criterion / machine a1b2c3d4 | 96.93 ns to 103.5 ns | 126.1 ns | +22.6 ns (+21.85%) |

Showing 1 of 1 findings by magnitude. Full Markdown and JSON retain every finding and complete identities.

<details>
<summary>regression: http_parse/case evidence</summary>

Platform / engine / machine: criterion / x86\_64-unknown-linux-gnu / machine a1b2c3d4

#### `http_parse/case`
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

</details>

