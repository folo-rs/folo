# examine

`examine` answers the question a finding raises: *which commits actually moved this number?*
Where [`analyze`](analyze.md) reports that a benchmark's metric shifted and draws a small
chart, `examine` pivots that chart into its data points — one row per recorded observation of
a single `(benchmark, metric)` series, in git first-parent order, each row pairing the value
with the short commit id and the start of the commit's title.

```console
cargo bench-history examine --local=./bench-history \
    --benchmark my_pkg/my_group/my_case --metric instruction_count
```

Two required options name the series, and this is the one command that names a **metric**:

- `--benchmark <qualified-id>` selects exactly one benchmark identity.
- `--metric <name>` selects one metric by its stable name.

`analyze` exposes no metric filter because you are not expected to know the internal metric
names — but `examine`'s input is an `analyze` *finding*, which already prints both the
benchmark identity and the metric, so pasting them back in is natural.

`examine` is a drill-down sibling of [`list runs`](list.md): both are read-only previews over
`analyze`'s exact data-set selection that never analyze. It runs **no detection and no
re-baselining** — it has no findings, modes, or blessings — and repeats the pivot once per
matching discriminant set. The text and Markdown renderings lead each set with a small
neutral line chart of the selected series (`examine` makes no judgment); the JSON form
carries the ordered points at full precision with each commit's full title.
