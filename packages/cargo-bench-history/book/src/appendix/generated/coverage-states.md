| State | Verdict on a silent run | What the silence covers |
|---|---|---|
| `no_series` | Nothing was analyzed, so no change could be detected. | Nothing at all. No series entered analysis; look at the empty-outcome hint. |
| `nothing_in_scope` | Nothing was in scope at the analyzed tip commit, so nothing was judged. | Nothing. Every series the store holds was measured somewhere other than the analyzed commit, so the run says nothing about that commit. |
| `nothing_judged` | Nothing was judged, so no change could be detected either way. | Nothing. Series were in scope and every one of them fell short of an evidence floor; the breakdown says which. |
| `partial` | No notable changes detected among the series that were judged. | The judged part only. Silence here rules out change among those series and makes no claim about the rest. |
| `full` | No notable changes detected. | The whole in-scope suite. This is the only state in which silence is an unqualified all-clear. |
