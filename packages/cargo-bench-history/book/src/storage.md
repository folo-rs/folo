# Storage backends

Every run is stored as an immutable record. Where those records live is chosen **at run
time**, not baked into the committed configuration:

- **Local filesystem** — pass `--local=<path>`, or a bare `--local` to take the path from
  the `CARGO_BENCH_HISTORY_STORAGE` environment variable.
- **Azure Blob** — configure a cloud backend in `.cargo/bench_history.toml` and omit
  `--local`.

A local path is machine-dependent, so it is **never** stored in the shared, version-controlled
config file. `--local` always overrides a configured cloud backend.

## Backend selection precedence

For every storage-backed command:

1. An explicit `--local=<path>`.
2. A bare `--local` — the path from the storage environment variable (unset or empty is an
   error).
3. Otherwise, the configured cloud backend.
4. Otherwise, a configuration error.

`collect --no-store` is the one exception: it skips selection entirely for dry runs.

## The Azure Blob backend

The cloud backend is configured in `.cargo/bench_history.toml` — run
[`install`](commands/install.md) to generate a fully commented starter file. Authentication
is via Microsoft Entra ID (OAuth); there are no static credentials in the config.

> **Note**
> The specifics of provisioning an Azure Blob container and the exact config schema are
> documented in the generated config file's comments. This page describes the selection
> model; follow the starter file for the concrete fields.

## Read-through cache (cloud backend, CI)

The read commands ([`analyze`](commands/analyze.md), [`examine`](commands/examine.md),
[`list`](commands/list.md), [`prune`](commands/prune.md)) load the whole in-selection
history before reconstructing a series. Against the cloud backend that is one download per
object, so CI would re-fetch everything on every run.

A `--cache <dir>` flag (with an environment fallback) enables an on-disk read-through cache
that mirrors fetched object bodies. Because stored records are immutable per key, a cached
body is trusted indefinitely; a small per-project marker invalidates the cache after the
rare delete or overwrite. The cache is meaningful only with the cloud backend, so it
**conflicts with `--local`**. In GitHub Actions, persist the cache directory with the
standard Actions cache so each run pays the network cost only for objects it has never seen.

## Continuous integration notes

- [`analyze`](commands/analyze.md) needs a resolvable git repository with enough history to
  find the merge-base with the base branch. On a shallow clone, deepen it
  (`git fetch --unshallow`, or `fetch-depth: 0` in `actions/checkout`) or pass an explicit
  `--base`.
- Run CI collection with `--skip-existing` for append-only behavior: a same-commit re-run
  still benchmarks every engine (so a broken benchmark is caught) but writes nothing,
  keeping caches valid across runs.
