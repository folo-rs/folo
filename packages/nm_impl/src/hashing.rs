use std::collections::HashMap as StdHashMap;
use std::hash::RandomState;

/// A `HashMap` that hashes with the standard library's `HashDoS`-resistant default.
///
/// The process-wide event registry and the collection path are keyed by event
/// names. `EventBuilder::name` accepts owned strings precisely so that names can
/// be derived at runtime, which puts the key set outside this crate's control:
/// it is neither bounded nor guaranteed to be free of external influence.
///
/// The choice rests on the documented contract, not on the seed being hard to
/// guess. The standard library states that its default `HashMap` algorithm is
/// selected to resist `HashDoS` attacks and is seeded from the host's secure
/// randomness. The faster `foldhash` describes itself as only minimally
/// DoS-resistant, tells callers not to rely on it for security, and seeds itself
/// from load addresses and the wall clock, so it answers a weaker question than
/// the one an adversarially chosen name set asks.
///
/// These maps are consulted when an event is registered and once per event per
/// report collection, never per observation, so the stronger hash lands on paths
/// that already do far more work per call. It raises the instruction counts of
/// Callgrind benchmarks that build or iterate these maps, and the random seed
/// makes those counts vary between processes.
/// Ref: workspace docs/benchmarks.md, "Hash containers and instruction-count
/// determinism".
pub(crate) type HashMap<K, V> = StdHashMap<K, V, RandomState>;
