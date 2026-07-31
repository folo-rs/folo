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

The key hashes only **properties of the hardware**: the processor and memory-region counts and
the distinct processor models present. Readings that the same machine can report differently
from one boot to the next — per-processor speeds above all, which are boot-time calibration
figures rather than hardware identity — are deliberately excluded. It takes very little drift
to do damage: a GitHub-hosted ARM64 Windows runner reports all four of its Cobalt 100
processors calibrated at 10678 on most boots and one of the four at 10681 on others, and
hashing that reading would fork the runner's history between the two. That is exactly the
fragmentation the key exists to prevent, and the speeds add no discriminating power the
processor models do not already carry. They are recorded with every run as provenance instead,
so a machine's speed mix is answered from its stored runs rather than from its key.

The key is version-tagged, so a change to the factors it hashes forks stored history into a
new partition rather than silently mixing incomparable data.
[`rekey`](rekey.md) closes such a fork by migrating the stored objects onto the new format.

See [Comparability and partitioning](../concepts/comparability.md) for how the machine key
fits into the discriminant set.
