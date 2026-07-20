# machine-key

Prints this machine's hardware fingerprint — the key every engine's history is partitioned
by.

```console
cargo bench-history machine-key
```

Every engine's numbers are machine-dependent in practice — Criterion wall-clock time and
`all_the_time` CPU time obviously, but Callgrind instruction counts and `alloc_tracker`
allocations too, because libraries dispatch to different code paths on different
microarchitectures. So every engine's history is partitioned by this machine key, and runs
from different machines are never mixed.

`--verbose` additionally reports the factors behind the key on standard error, for tracing a
key change to the hardware detail that moved.

See [Comparability and partitioning](../concepts/comparability.md) for how the machine key
fits into the discriminant set.
