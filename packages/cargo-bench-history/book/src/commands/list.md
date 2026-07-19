# list

`list` previews the exact data set an [`analyze`](analyze.md) pass would consume, without
running the analysis, letting you confirm the commit range and discriminant sets first. It
takes a bare positional subject — `runs`, `discriminants`, or `blessings`. A bare `list` with
no subject is an error that names the three.

```console
# Per discriminant set, the run / series / per-commit counts of the selected runs.
cargo bench-history list runs --local=./bench-history

# A discovery catalog of the discriminant sets present in storage (needs no repository).
cargo bench-history list discriminants --local=./bench-history

# Audit blessings.
cargo bench-history list blessings --local=./bench-history
```

## Subjects

- **`runs`** — mirrors `analyze`'s data-set-selection parameters exactly through the shared
  selection pipeline, and reports, per discriminant set, the run / series / per-commit counts
  of the selected runs (each commit's clean/dirty split), oldest-first by topology.
- **`discriminants`** — a different view: a discovery catalog of the sets present in storage.
  It requires **no repository** and so ignores the timeline and data-filtering groups. With no
  facets it lists every stored partition, so you can find triples and machine keys you do not
  already know.
- **`blessings`** — audits blessings (see [bless / unbless](bless.md)): the sidecars at the
  current commit by default. Add `--all` to show the most recent blessing of every
  benchmark across the analysis window.
