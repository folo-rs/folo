# machine-key

Prints this machine's hardware fingerprint — the key that hardware-dependent history is
partitioned by.

```console
cargo bench-history machine-key
```

Hardware-dependent engines (Criterion wall-clock time, `all_the_time` CPU time) are only
comparable across runs on the same class of hardware, so their history is partitioned by this
machine key. Hardware-independent engines (Callgrind instruction counts, `alloc_tracker`
allocations) use a literal `synthetic` placeholder instead.

`--verbose` additionally reports the factors behind the key on standard error, for tracing a
key change to the hardware detail that moved.

See [Comparability and partitioning](../concepts/comparability.md) for how the machine key
fits into the discriminant set.
