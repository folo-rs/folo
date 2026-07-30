# examine

`examine` answers the question a finding raises: *which commits actually moved this number?*
Where [`analyze`](analyze.md) reports that a benchmark's metric shifted and draws a small
chart, `examine` pivots that chart into a per-commit listing of a single `(benchmark, metric)`
series — a row for every commit, in git first-parent order, each row pairing the value with
the short commit id and the start of the commit's title.

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
re-baselining** — it has no findings, modes, or blessings — and repeats the listing once per
matching discriminant set.

The listing covers **every commit in the examined range**: from the earliest commit at which
any matching set carries the series through to the analyzed tip. Every set shares that range,
so their tables cover the same commits and can be read side by side. A commit that carries
data contributes one row per observation (clean before dirty, each flagged); a commit with no
data point reads `n/a`, still naming the commit and its title. Nothing caps the listing — use
`--since` to bound the range.

The JSON form carries the same rows in the same order, with full precision and each commit's
full title (the text and Markdown tables truncate the title to 50 characters). A row for a
commit with no data point has a null value and no clean/dirty flag.

The text and Markdown renderings lead each set with the same compact, topology-accurate line
chart history-mode `analyze` draws — one column per first-parent commit over that set's own
observations, so a data-less commit is a gap. The chart trims its own leading gap, so a
late-starting set draws a chart that begins after its table does: the table is the complete
commit listing, the chart is the shape of the series.
