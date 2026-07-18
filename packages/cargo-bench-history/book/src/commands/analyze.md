# analyze

`analyze` pieces a series together at query time from git topology, so it requires a
resolvable git repository (the current checkout by default, or an explicit path). With no
repo it errors rather than guessing an order.

```console
cargo bench-history analyze --local=./bench-history
```

## Target, base, and modes

Two refs frame the analysis: a **target** (`--context`, default `HEAD`) whose history is
analyzed, and a **base** (`--base`, default the detected default branch). `analyze` resolves
the first-parent ancestry of the target and splits it at the merge-base with the base.

From that topology `analyze` auto-selects one of two **modes** — there is no flag to force
one:

- **history** — the base-branch view (the analyzed tip *is* the merge-base). It applies
  long-range change-point detection, drift detection, and false-discovery correction, and
  reports regressions only by default.
- **branch** — the feature-branch view (commits past the merge-base). It judges the branch by
  its tip commit's latest state against the base and reports both regressions and improvements.
  Only the tip commit lands in the base on merge, so the branch's own intermediate history is
  ignored.

See [Analysis](../concepts/analysis.md) for what each mode detects.

> **Shallow clones**
> If the base cannot be resolved or shares no common ancestor with the target — typically a
> shallow clone — `analyze` errors and points at the fix (`git fetch --unshallow` /
> `fetch-depth: 0`, or an explicit `--base`) rather than guessing.

## Selecting the window and scope

- `--since` drops whole runs older than the cutoff by each commit's committer date. It
  defaults to a six-month look-back, so a scheduled trend watch does not silently widen as
  history accumulates.
- Positional prefix subjects scope the analysis to benchmarks whose id starts with a prefix.
  There is no metric filter — metrics are an internal detail.
- **Ghost** benchmarks (identities with no run at the context commit — deleted, renamed, or
  replaced) are always dropped before detection, since re-flagging a benchmark that no longer
  exists is noise.

## Output

Text goes to stdout by default. File toggles compose, so a single pass can emit text,
Markdown, and JSON at once; requesting no output at all is an error. A derived, condensed
Markdown **summary** is also available for a size-limited downstream consumer.

**Findings never affect the exit code**: the process exits non-zero only when the analysis
fails to *run*. A finding is advisory; the machine-readable signal lives in the JSON report,
which downstream automation should read rather than the exit status.

Every query run also prints a one-line effective-selection summary to stderr (engine,
target-triple, and machine-key facets, the resolved base branch, and the `--since` cutoff),
so you always see what was actually searched.
