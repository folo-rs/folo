```text
run
├── schema_version          5
├── context
│   ├── observed_at         when the measurement was taken (provenance only)
│   ├── git                 commit, branch, and whether the tree was dirty
│   ├── environment         local, or the CI provider that ran it
│   ├── toolchain           target triple and compiler version
│   ├── tool_version        which cargo-bench-history wrote this
│   ├── machine             host hardware description (recorded, never read)
│   └── best_of             repetitions run, when --best-of was used
└── results[]               one per benchmark case
    ├── id                  the benchmark identity
    └── metrics[]           one per metric kind, each with
        ├── value           the measurement
        ├── std_dev         where the engine reports one (recorded, never read)
        └── interval        low and high, where the engine reports them
```
