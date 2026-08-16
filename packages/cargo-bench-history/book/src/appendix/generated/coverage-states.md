| State | Verdict on a silent run | What the silence covers |
|---|---|---|
| `no_series` | Nothing was analyzed, so no change could be detected. | Nothing: no series was accounted for. The empty-outcome hint explains why. |
| `nothing_in_scope` | Nothing was in scope at the analyzed tip commit, so nothing was judged. | Nothing at the analyzed commit: every accounted series was measured elsewhere. |
| `nothing_judged` | Nothing was judged, so no change could be detected either way. | Nothing: in-scope series existed but none could be judged; the breakdown says which evidence floor they fell short of. |
| `partial` | No notable changes detected among the series that were judged. | The judged series only: no reportable move among them, and no claim about the in-scope series that went unjudged. |
| `full` | No notable changes detected. | The whole in-scope suite: every in-scope series was judged, so this is the only silent state with no coverage qualification. The verdict stays no notable changes detected. |
