# cbh_model - Implementation

`cbh_model` implements the stored data and comparability concepts described by the
[`cargo-bench-history` design](../../cargo-bench-history/docs/DESIGN.md). It owns benchmark
identity, metrics, run context, storage-key components, blessings, and the pure best-of-N
reduction.

The crate is I/O-free. Engine adapters construct the model, storage codecs serialize it, and
analysis consumes it without introducing dependencies on those layers. This keeps model
invariants inexpensive to test under Miri and mutation testing.

Best-of-N APIs retain their public aggregate return types. Missing-case, missing-metric, and empty
prefix values are represented by private semantic conditions beneath those aggregates. Exact
condition fields are tested inside the defining modules; downstream code depends on aggregate
behavior rather than private reduction details.
