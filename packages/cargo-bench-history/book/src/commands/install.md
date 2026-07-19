# install

Generates a fully commented example configuration file (`.cargo/bench_history.toml`) if one
is absent, and points you at it. It never clobbers an existing file.

```console
cargo bench-history install
```

The template documents the optional cloud backend and notes that local storage is selected
at run time (flag or environment), not configured in the committed file. It carries no
engine or machine-key settings, and its next-steps hint points at
[`backfill`](backfill.md) for seeding an existing repository's history.
