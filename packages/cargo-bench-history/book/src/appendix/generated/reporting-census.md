A report with nothing to say still says how far that silence reaches:

```text
Analyzed project textproc (history mode)
  commit: 9f2c4a1d3b5e708aab12cd34ef5678901234567a
  runs: 128 (4d17b0c93ea2 → 9f2c4a1d3b5e)  in-scope series judged: 3 of 6  regressions: 0
No notable changes detected among the series that were judged.
  Judged 3 of 6 in-scope series; no reportable move survived the gates.
  Not judged: 2 series not measured at the analyzed tip commit; 3 series with too few points in the analyzed window.
```

| Part | How to read it |
|---|---|
| `in-scope series judged: 3 of 6` | The denominator of every claim the report makes. It counts series the analysis could have judged, which is every series it accounted for except the ghosts. |
| `No notable changes detected among the series that were judged.` | The verdict. It is a statement about the judged series alone, and its wording changes with how much of the suite that is. |
| `Judged 3 of 6 in-scope series; no reportable move survived the gates.` | How far the verdict reaches: the share of the in-scope suite it is a statement about. |
| `Not judged: 2 series not measured at the analyzed tip commit; 3 series with too few points in the analyzed window.` | What the verdict does not cover, named reason by reason. The judged count and these reasons account for every series between them. |

The judged ratio heads every report that had anything in scope. The per-reason breakdown is printed by the text and Markdown reports only where the report has no findings, as here; the JSON report always carries it, under `census.reasons`.
