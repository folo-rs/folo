use foldhash::fast::FixedState;

/// A `HashMap` seeded with a fixed hash key for deterministic iteration.
///
/// The process-wide event registry and the collection path use this instead of
/// the default `foldhash` map. The default `RandomState` derives its seed from
/// process memory addresses, so both the iteration order and the number of
/// internal probe/resize operations differ from one process to the next. That
/// nondeterminism is invisible to ordinary callers but surfaces as
/// instruction-count jitter in Callgrind benchmarks that build or iterate these
/// maps: byte-identical source produces a different count on every build. A
/// fixed seed removes that variance.
///
/// Event names are internal identifiers rather than attacker-controlled input,
/// so forgoing the default `HashDoS` resistance carries no risk here.
pub(crate) type HashMap<K, V> = std::collections::HashMap<K, V, FixedState>;
