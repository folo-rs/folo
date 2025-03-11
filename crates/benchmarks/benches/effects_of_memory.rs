use std::{
    collections::HashMap,
    hint::black_box,
    sync::{Arc, RwLock, mpsc},
};

use criterion::{Criterion, criterion_group, criterion_main};
use fake_headers::Headers;
use frozen_collections::{FzHashMap, FzScalarMap, MapQuery};
use http::{HeaderMap, HeaderName, HeaderValue};
use many_cpus_benchmarking::{Payload, WorkDistribution, execute_runs};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

criterion_group!(benches, entrypoint);
criterion_main!(benches);

// TODO: Do not hang forever if worker panics. We need timeouts on barriers!

/// A "small" data set is likely to fit into processor caches, demonstrating the effect of memory
/// access on typically cached data).
const SMALL_MAP_ENTRY_COUNT: usize = 64 * 1024; // x u64 = 512 KB of useful payload, cache-friendly

/// The small maps fit into memory very easily. 512 KB * 1000 = 512 MB per worker.
const SMALL_MAP_BATCH_SIZE: u64 = 1000;

/// A large data set is unlikely to fit into processor caches, even into large L3 caches, and will
/// likely require trips to main memory for repeated access.
const LARGE_MAP_ENTRY_COUNT: usize = 16 * 128 * 1024; // x u64 -> 128 MB, not very cache-friendly.

/// The large maps are large, so we try conserve memory. 128 MB * 10 = 1.28 GB per worker.
const LARGE_MAP_BATCH_SIZE: u64 = 10;

fn entrypoint(c: &mut Criterion) {
    // This payload is only 10K items, so we use a large batch size as it fits well in memory.
    execute_runs::<ChannelExchange, 1000>(c, WorkDistribution::all_with_unique_processors());

    execute_runs::<HashMapRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        // We do not care about "self" because the workers both perform the same work anyway, so
        // it does not matter whether the payload is exchanged or not.
        WorkDistribution::all_with_unique_processors_without_self(),
    );

    execute_runs::<HashMapBothRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        // We do not care about "self" because the workers allocate roles dynamically at runtime,
        // so it does not matter whether the payload is exchanged or not.
        WorkDistribution::all_with_unique_processors_without_self(),
    );

    execute_runs::<FzHashMapRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    // This repeatedly reads the same map 10 times per iteration, to answer the question: does
    // local caching eliminate the impact of reading from a different memory on the Nth read?
    // If we still see a difference between same-region and cross-region reads, there is still
    // benefit in keeping data local. If we see no difference, it is evidence that it matters less
    // for data that is already locally cached.
    //
    // Another effect this may show us is the effect of caching within the same region (on different
    // processors) - with the `Unpinned` distribution modes, the workload may move between
    // processors, resulting in cache misses that should slow things down even if same-region.
    execute_runs::<FzHashMapRead<SMALL_MAP_ENTRY_COUNT, 10>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    execute_runs::<FzScalarMapRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    // This repeatedly reads the same map 10 times per iteration, to answer the question: does
    // local caching eliminate the impact of reading from a different memory on the Nth read?
    // If we still see a difference between same-region and cross-region reads, there is still
    // benefit in keeping data local. If we see no difference, it is evidence that it matters less
    // for data that is already locally cached.
    //
    // Another effect this may show us is the effect of caching within the same region (on different
    // processors) - with the `Unpinned` distribution modes, the workload may move between
    // processors, resulting in cache misses that should slow things down even if same-region.
    execute_runs::<FzScalarMapRead<SMALL_MAP_ENTRY_COUNT, 10>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    // We also include the same-processor variant here because it shows a surprising speedup.
    // Reason unknown but this scenario has a lot of memory allocation in HTTP parsing, which
    // is unusual compared to the others. Maybe related, maybe not.
    //
    // We use a large batch size, as the payload is quite small (10K headers) and fits well in memory.
    execute_runs::<HttpHeadersParse, 1000>(c, WorkDistribution::all());

    execute_runs::<SccMapRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    execute_runs::<SccMapBothRead<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        WorkDistribution::all_with_unique_processors(),
    );

    execute_runs::<SccMapSharedReadWrite<SMALL_MAP_ENTRY_COUNT, 1>, SMALL_MAP_BATCH_SIZE>(
        c,
        // We do not care about "self" because the workers allocate roles dynamically at runtime,
        // so it does not matter whether the payload is exchanged or not.
        WorkDistribution::all_without_self(),
    );

    // This is extremely slow due to giant payload but what can we do, the giant payload is the
    // point of this scenario - to show what happens when data that does not fit in caches.
    execute_runs::<SccMapSharedReadWrite<LARGE_MAP_ENTRY_COUNT, 1>, LARGE_MAP_BATCH_SIZE>(
        c,
        // We do not care about "self" because the workers allocate roles dynamically at runtime,
        // so it does not matter whether the payload is exchanged or not.
        WorkDistribution::all_without_self(),
    );
}

/// The two paired workers exchange messages with each other, sending back whatever is received.
/// Note: SameProcessor mode is not supported because the workers must collaborate in real time.
#[derive(Debug)]
struct ChannelExchange {
    // Anything received here...
    rx: mpsc::Receiver<u64>,

    // ...is sent back here.
    tx: mpsc::Sender<u64>,
}

impl Payload for ChannelExchange {
    fn new_pair() -> (Self, Self) {
        let (c1_tx, c1_rx) = mpsc::channel::<u64>();
        let (c2_tx, c2_rx) = mpsc::channel::<u64>();

        let worker1 = Self {
            rx: c1_rx,
            tx: c2_tx,
        };
        let worker2 = Self {
            rx: c2_rx,
            tx: c1_tx,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        const INITIAL_BUFFERED_MESSAGE_COUNT: usize = 10_000;

        for i in 0..INITIAL_BUFFERED_MESSAGE_COUNT {
            self.tx.send(i as u64).unwrap();
        }
    }

    fn process(&mut self) {
        const MESSAGE_PUMP_ITERATION_COUNT: usize = 500_000;

        for _ in 0..MESSAGE_PUMP_ITERATION_COUNT {
            let payload = self.rx.recv().unwrap();

            // It is OK for this to return Err when shutting down (the partner worker has already
            // exited and cleaned up, so the message will never be received - which is fine).
            _ = self.tx.send(payload);
        }
    }
}

/// The first worker generates a hashmap with MAP_ENTRY_COUNT entries.
/// The other worker reads all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct HashMapRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: HashMap<u64, u64>,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for HashMapRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.map = HashMap::with_capacity(MAP_ENTRY_COUNT);

        for i in 0..MAP_ENTRY_COUNT {
            self.map.insert(i as u64, (i * 2) as u64);
        }
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(self.map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates a hashmap with MAP_ENTRY_COUNT entries.
/// Both workers read all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct HashMapBothRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: Arc<RwLock<HashMap<u64, u64>>>,

    // Only needs to be filled by one of the workers, where is is true.
    is_filler: bool,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for HashMapBothRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        let map = Arc::new(RwLock::new(HashMap::with_capacity(MAP_ENTRY_COUNT)));

        let worker1 = Self {
            map: Arc::clone(&map),
            is_filler: true,
        };
        let worker2 = Self {
            map,
            is_filler: false,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        if !self.is_filler {
            return;
        }

        let mut map = self.map.write().unwrap();

        for i in 0..MAP_ENTRY_COUNT {
            map.insert(i as u64, (i * 2) as u64);
        }
    }

    fn process(&mut self) {
        let map = self.map.read().unwrap();

        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates a frozen hashmap with MAP_ENTRY_COUNT entries.
/// The second worker reads all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct FzHashMapRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: FzHashMap<u64, u64>,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for FzHashMapRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        let entries = (0..MAP_ENTRY_COUNT)
            .map(|i| (i as u64, (i * 2) as u64))
            .collect();

        self.map = FzHashMap::new(entries);
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(self.map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates a frozen scalar map with MAP_ENTRY_COUNT entries.
/// The second worker reads all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct FzScalarMapRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: FzScalarMap<u64, u64>,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for FzScalarMapRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        let entries = (0..MAP_ENTRY_COUNT)
            .map(|i| (i as u64, (i * 2) as u64))
            .collect();

        self.map = FzScalarMap::new(entries);
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(self.map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates an SCC hashmap with MAP_ENTRY_COUNT entries.
/// The second worker reads all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct SccMapRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: scc::HashMap<u64, u64>,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for SccMapRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.map = scc::HashMap::with_capacity(MAP_ENTRY_COUNT);

        for i in 0..MAP_ENTRY_COUNT {
            self.map.insert(i as u64, (i * 2) as u64).unwrap();
        }
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(self.map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates an SCC hashmap with MAP_ENTRY_COUNT entries.
/// Both workers read all elements from it REPEAT_COUNT times.
#[derive(Debug, Default)]
struct SccMapBothRead<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: Arc<scc::HashMap<u64, u64>>,

    // Only needs to be filled by one of the workers, where is is true.
    is_filler: bool,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for SccMapBothRead<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        let map = Arc::new(scc::HashMap::with_capacity(MAP_ENTRY_COUNT));

        let worker1 = Self {
            map: Arc::clone(&map),
            is_filler: true,
        };
        let worker2 = Self {
            map,
            is_filler: false,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        if !self.is_filler {
            return;
        }

        for i in 0..MAP_ENTRY_COUNT {
            self.map.insert(i as u64, (i * 2) as u64).unwrap();
        }
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            for k in 0..MAP_ENTRY_COUNT {
                black_box(self.map.get(&(k as u64)));
            }
        }
    }
}

/// The first worker generates an SCC hashmap with MAP_ENTRY_COUNT entries.
/// Both workers increment entries in the same shared map, one worker from high to low, the other
/// from low to high, to avoid conflicting on the same keys. We want to see data effects, not lock
/// contention. The increment loop is repeated REPEAT_COUNT times.
#[derive(Debug, Default)]
struct SccMapSharedReadWrite<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> {
    map: Arc<scc::HashMap<u64, u64>>,

    // Determines whether it does the initial fill and which direction it iterates.
    is_worker_1: bool,
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize>
    SccMapSharedReadWrite<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn increment(&self, key: u64) {
        let mut value = black_box(self.map.get(&key).unwrap());
        *value += 1;
    }
}

impl<const MAP_ENTRY_COUNT: usize, const REPEAT_COUNT: usize> Payload
    for SccMapSharedReadWrite<MAP_ENTRY_COUNT, REPEAT_COUNT>
{
    fn new_pair() -> (Self, Self) {
        let map = Arc::new(scc::HashMap::with_capacity(MAP_ENTRY_COUNT));

        let worker1 = Self {
            map: Arc::clone(&map),
            is_worker_1: true,
        };
        let worker2 = Self {
            map,
            is_worker_1: false,
        };

        (worker1, worker2)
    }

    fn prepare(&mut self) {
        if !self.is_worker_1 {
            return;
        }

        for i in 0..MAP_ENTRY_COUNT {
            self.map.insert(i as u64, (i * 2) as u64).unwrap();
        }
    }

    fn process(&mut self) {
        for _ in 0..REPEAT_COUNT {
            if self.is_worker_1 {
                for key in 0..MAP_ENTRY_COUNT as u64 {
                    self.increment(key);
                }
            } else {
                for key in (0..MAP_ENTRY_COUNT as u64).rev() {
                    self.increment(key);
                }
            }
        }
    }
}

const HEADERS_COUNT: usize = 10_000;

// One worker serializes HTTP headers, the other deserializes them back.
// Involves a fair bit of memory allocation but somewhat "realistic".
#[derive(Debug, Default)]
struct HttpHeadersParse {
    serialized: Option<Vec<Vec<u8>>>,
}

impl Payload for HttpHeadersParse {
    fn new_pair() -> (Self, Self) {
        (Self::default(), Self::default())
    }

    fn prepare(&mut self) {
        self.serialized = Some(
            (0..HEADERS_COUNT)
                .map(|_| {
                    let headers = Headers::default().generate();

                    let mut result = String::new();

                    for (key, value) in headers {
                        result.push_str(&format!("{key}: {value}\r\n"));
                    }

                    result.into_bytes()
                })
                .collect(),
        );
    }

    fn process(&mut self) {
        for serialized in self.serialized.take().unwrap() {
            // SAFETY: We serialized proper utf-8, it is fine.
            let serialized_str = unsafe { String::from_utf8_unchecked(serialized) };

            let mut headers = HeaderMap::new();
            for line in serialized_str.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let key = key.trim();
                    let value = value.trim();
                    let header_name = HeaderName::from_bytes(key.as_bytes()).unwrap();
                    let header_value = HeaderValue::from_str(value).unwrap();
                    headers.insert(header_name, header_value);
                }
            }

            black_box(headers);
        }
    }
}
