# `analyze` data-flow & parallelism reference

A mental model of the `analyze` pipeline: what loads where, in what order, what is
computed/sorted, and exactly where concurrency vs. parallelism happens. This is the
canonical reference for the load and detection path; keep it in sync with the logic
(see `AGENTS.md`, the `analyze` section). For the *statistical* design (detectors,
re-baselining semantics) see `DESIGN.md`; this document is about *flow and
performance*.

Code lives in two crates, referenced by symbol + file:

- **shell** `cargo-bench-history` — IO, git, storage, orchestration
  (`src/analyze/mod.rs`, `src/storage/`).
- **core** `cargo-bench-history-core` — pure compute leaves
  (`src/analyze/{series,stats,findings,parallel,report}.rs`).

---

## 1. Two kinds of "going wide" (read this first)

The single most important distinction:

| Mechanism | What it is | Where | Threads |
|---|---|---|---|
| **I/O concurrency** | `buffer_unordered(LOAD_CONCURRENCY)` multiplexes in-flight `Storage::get` futures on **one** `!Send` task | the fetch loop in `select_dataset` | cooperative on a single task — **not** OS-thread parallelism |
| **CPU parallelism** | one balanced contiguous chunk per worker is dispatched to a blocking task through an injected `anyspawn::Spawner` (`spawn_blocking`); chunk math in `analyze::parallel` | `find_changes_spawned` (the detection step) | runtime worker threads, not freshly spawned per call |

"Fetching N at once" and "detecting across cores" are *different machines*. The fetch
is latency-hiding I/O on one logical task; the **detection** stage is the only place
that fans compute out across cores, and it does so through a `Spawner` so the work
runs on the runtime's shared blocking pool (production) — or inline on the calling
thread (tests/Miri) — rather than on anonymous per-call OS threads. The load future
is `!Send` and reactor-free, so it runs unchanged under the Miri-driven
`futures::executor::block_on` tests; the production binary runs it under
`#[tokio::main]`, and that same runtime backs the detection spawner.

---

## 2. Top-level flow (one `analyze` invocation)

```mermaid
flowchart TD
  EXEC["analyze::execute()"] --> AW["analyze_with()"]
  AW --> SD["select_dataset()"]
  SD --> DS[("SelectedDataSet:\nseries + run_index + blessings")]
  DS --> AB["apply_blessings()\n(history mode re-baseline)"]
  AB --> FC["find_changes_spawned()"]
  FC --> SUM["per-set summaries"]
  SUM --> RENDER["render()"]
  RENDER --> OUT["RunOutcome::Analyzed\nreport + regressions"]
```

The cost is overwhelmingly in **`select_dataset`** (the load) and secondarily in
**`find_changes_spawned`** (the detect). Everything else is bookkeeping.

Each analysis **mode** (`history`, `branch`, `tip`) is a *separate* `analyze`
invocation with its *own* `select_dataset` load — there is no shared dataset cache
across modes. The mode is auto-detected once per run from git topology
(`auto_mode`), unless `--mode` overrides it.

---

## 3. `select_dataset` — the load (where the wall-clock goes)

```mermaid
flowchart TD
  subgraph P1["Phase 1 — key-only filtering (NO payload fetched)"]
    L["storage.list(prefix)\n1 round-trip, returns all keys"] --> KF["parse_key + facet match"]
    KF --> WF["history / dirty / since-until filters\n(commit time + topology)"]
    WF --> TF["to_fetch: Vec<(key, StorageKey)>"]
    TF --> SK["sort by storage key\n→ each object's rank = object_ordinal"]
  end
  SK --> LOOP
  subgraph P23["Phase 2/3 — fetch + fold, streaming"]
    LOOP["buffer_unordered(LOAD_CONCURRENCY)\nfetch_one_ranked → Storage::get → JSON → Run"] -->|"Run, OUT of order"| FOLD["serial fold: SeriesBuilder.push per run\nintern commit, hash id, group points"]
    FOLD --> DROP["drop parsed Run\nkeep only compact SeriesPoints"]
    DROP --> LOOP
  end
  DROP --> FIN["SeriesBuilder.finish()"]
```

Key properties:

- **Phase 1 never fetches a payload.** History-membership, base-side dirty admission
  and the `--since`/`--until` window are all decided from the *key* and git topology
  (`window_excludes`), so an excluded object costs zero round-trips.
- **Ordinals are assigned up front** by sorting `to_fetch` by key, because
  `buffer_unordered` completes out of order; this makes the result independent of
  fetch arrival order. `object_ordinal` is the final point tie-break and stands in for
  the full storage key to keep points small.
- **Streaming memory.** Only the objects currently in flight (raw bytes + the one
  parsed `Run` being folded) are resident; each `Run` is dropped right after its
  points are extracted. On a large history this is the difference between hundreds of
  MB and tens of GB.
- **Fetch is concurrent, parse + fold are serial.** Each completed fetch is parsed
  (JSON → `Run`) and folded (`SeriesBuilder::push`) inline on the single load task as
  it arrives, overlapped with the still-in-flight fetches. The fold is a sequential
  section, but it overlaps the concurrent fetch pipeline, so on the remote backend it
  hides under fetch latency rather than gating it; an earlier per-batch parallel-parse
  scheme was dropped because its chunks were too small to beat the fork/join overhead
  (see §6).

### `SeriesBuilder::push` — what the fold does (`analyze::series`)

Per parsed `Run`: intern the short commit into an `Arc<str>` shared by every point on
that commit (`intern`); resolve the benchmark id's bucket in a `HashTable` with a
single `entry` probe — hashing `BenchmarkId` with the builder's one fixed hasher
instance and **cloning the id only on a true cache miss** (one id-clone per distinct
series, never per point); push a compact `SeriesPoint` (`topo_index`, `dirty`,
`object_ordinal: u32`, `commit: Arc<str>`, `value`, `interval_low/high`) into the
`(set, id, kind)` group. A large history materialises tens of millions of these,
hence the compactness and interning.

### Tuning constants (`analyze::mod`)

- `LOAD_CONCURRENCY` — how many `Storage::get` round-trips overlap. Hides per-object
  latency (critical on the remote backend); set near the knee where in-flight fetches
  saturate the network path, past which extra concurrency only subdivides the fixed
  path bandwidth and lengthens each request without lifting throughput.

---

## 4. `SeriesBuilder::finish()` + `find_changes_spawned()` — build & detect

```mermaid
flowchart TD
  FIN["finish()"] --> FLAT["flatten nested maps → Vec<Series>"]
  FLAT --> SS["sort series by (set,id,kind)\n— SERIAL —"]
  SS --> PPS["sort each series' points by topology\n— SERIAL —"]
  PPS --> SERIES[("Vec<Series> → Arc<[Series]>")]
  SERIES --> DALL["detect_all_spawned: one chunk per worker\n→ spawn_blocking(detect_one)\n★ SPAWNED BLOCKING TASKS ★"]
  DALL --> CANDS["Vec<Candidate>"]
  CANDS --> BH["benjamini_hochberg FDR filter\n— SERIAL —"]
  BH --> MAT["materialise surviving findings'\nchart points"]
  MAT --> SF["sort findings by |Δ|, method, identity\n— SERIAL —"]
  SF --> FINDINGS[("Vec<Finding>")]
```

### Inside one `detect_one` (`analyze::findings`) — per series, runs on a worker

The detection step has no cross-series state, so the series are split into one
balanced contiguous chunk per worker and each chunk is detected on its own blocking
task; the chunks recombine in series order, so the output is identical to a sequential
pass. The mode selects the detector:

- **History** (long-range trend): project point values once, then run a
  **change-point** detector *and* a **drift** detector and keep the better fit
  (`arbitrate`); optionally a recovered-spike pass when inactive findings are
  requested.
- **Branch**: compare the branch tip's level against its base across the merge-base.
- **Tip**: guard only the newest point.

These detectors call the **stats kernels**, per series:

| Kernel | File | Cost | Allocation |
|---|---|---|---|
| `median_in_place` | `analyze::stats` | `sort_unstable_by(f64::total_cmp)` then midpoint | **none** — sorts the caller's slice in place, no scratch buffer |
| `theil_sen_line` | `analyze::stats` | `O(n²)` pairwise slopes, two `median_in_place`s | sizes its slope/intercept buffers once up front (`pair_count`) |
| `benjamini_hochberg` | `analyze::stats` | one `sort_unstable_by` over the p-values | once, across all noisy candidates |

`median_in_place` is genuinely in-place: the unstable sort orders without the scratch
buffer a stable sort would allocate, and ties under `total_cmp` are bit-identical so
reordering them cannot change the median.

---

## 5. The full parallelism / serial map

| Stage | Concurrency type | Unit of work | Fork/join criterion |
|---|---|---|---|
| `storage.list` | single async request | the whole prefix | — |
| Phase-1 filtering | serial | per candidate key | — |
| **fetch** | **I/O-concurrent (one task)** | per object, bounded in flight | `buffer_unordered(LOAD_CONCURRENCY)` |
| parse | **serial** (on the load task) | per object, as it arrives | — |
| fold (`push`) | **serial** | per `Run` | overlaps the concurrent fetch |
| series sort | serial | the `Vec<Series>` | — |
| point sort | serial | per series | — |
| **detect** | **CPU-parallel (spawned blocking tasks)** | one chunk of series per worker | split once over all series, await + concat |
| BH filter + finding sort + render | serial | the candidate/finding list | — |

`find_changes_spawned` splits the series into **exactly one balanced chunk per
worker** (sizes differ by at most one, so a slice just above the worker count still
uses every worker rather than collapsing to fewer chunks), dispatches each chunk to a
blocking task through the injected `Spawner`, then awaits and concatenates them in
series order — identical output to a sequential pass. A single available CPU (Miri
reports one) or a single series takes the serial path, dispatching no task.

---

## 6. Where the bottlenecks live (to steer optimization)

- **Local-filesystem backend:** the load is **fetch + serial parse/fold** bound. The
  fold runs inline as each object arrives, overlapped with the concurrent fetch, so
  the ceiling is fetch arrival plus the serial fold throughput. Levers: shrink the
  serial section (cheaper `push`), fewer/larger objects. A previous per-batch
  parallel-parse scheme was reverted: its chunks were too small to beat the fork/join
  overhead, and the fold already overlapped the fetch pipeline, so it added complexity
  for no measurable win.
- **Azure backend:** network-bound. With the shared connection pool (§7) the per-object
  TCP+TLS handshake is amortised across a keep-alive pool, leaving two ceilings: the
  path bandwidth between the client and the storage region, and — for small objects — a
  per-request round-trip rate, since each object costs one round-trip. Concurrency past
  the knee (`LOAD_CONCURRENCY`) only subdivides the fixed bandwidth and inflates latency
  tails, so fewer-larger blobs (and fewer bytes) help more than more concurrency.
- **Data-structure hot spots:** `SeriesPoint` compactness, `Arc<str>` commit interning
  and single-probe `HashTable` id lookups (clone-on-miss) keep the
  tens-of-millions-of-points fold affordable. Preserve these; do not regress them.

---

## 7. Azure connection reuse

The Azure backend (`storage::azure::AzureBlobStorage`) needs a separate per-object
`BlobClient` (and a `BlobContainerClient` for `list`) to address each blob, but they
all share **one** pooled HTTP client so every `get`/`put`/`delete`/`list` reuses a
single `reqwest` connection pool.

The reuse matters because `reqwest` pools connections *inside each `Client`*. If each
per-object client built its own transport — which is what `BlobClient::new(url,
credential, None)` does (`None` options → `Transport::default()` →
`new_http_client(None)` → a brand-new `reqwest::Client`) — every object would pay a
fresh TCP+TLS handshake with **no HTTP keep-alive reuse**. Raising fetch concurrency
would not help (each object still pays full connection setup) and at high concurrency
would exhaust ephemeral ports.

```mermaid
flowchart LR
  subgraph NOW["Client per object — no reuse"]
    direction TB
    A1["get A"] --> AC1["BlobClient::new(None)"] --> AR1["new reqwest::Client\n(own pool)"] --> AH1["TCP+TLS handshake"] --> AZ[("Azure")]
    B1["get B"] --> BC1["BlobClient::new(None)"] --> BR1["new reqwest::Client\n(own pool)"] --> BH1["TCP+TLS handshake"] --> AZ
  end
  subgraph FIX["Shared pooled client — keep-alive"]
    direction TB
    A2["get A"] --> SP["shared Arc<dyn HttpClient>\n(one pooled reqwest::Client)"]
    B2["get B"] --> SP
    SP --> KH["reused keep-alive connection"] --> AZ2[("Azure")]
  end
```

**How it is wired:** `AzureBlobStorage::from_config` builds **one** pooled HTTP client
(stored as the `http_client: Arc<dyn HttpClient>` field) and `shared_client_options()`
injects it into every per-object client through the transport seam, so all operations
share a single connection pool. The relevant symbols are re-exported from
`azure_core::http` (`new_http_client`, `HttpClientOptions`, `Transport`, `HttpClient`,
`ClientOptions`):

```rust
// built once in from_config:
let http_client = new_http_client(Some(HttpClientOptions {
    // The storage layer stores gzip and inflates it itself in `get`, so the
    // transport must hand back raw compressed bytes. This mirrors the SDK's own
    // per-client default; turning auto-decompression on would double-inflate.
    automatic_decompression: false,
}));

// per client (via shared_client_options):
let options = BlobClientOptions {
    client_options: ClientOptions {
        transport: Some(Transport::new(Arc::clone(&self.http_client))),
        ..Default::default()
    },
    ..Default::default()
};
BlobClient::new(url, self.credential.clone(), Some(options))
```

`automatic_decompression` must stay **off**: the SDK's own per-client transport sets it
to `false` and the storage layer inflates gzip manually in `get` (`codec::decompress`).
A shared client built with `new_http_client(None)` would default it to `true`, so
`reqwest` would auto-inflate and the manual `codec::decompress` would then double-inflate
the bytes.

This shared pool gives keep-alive connection reuse (far fewer handshakes, lower
per-object latency, no port exhaustion) and lets fetch concurrency actually pay off on
the remote backend. Validated by the Azurite round-trip tests (`storage::azure`) and the
real-Azure end-to-end tests (`cbh_azure::*_in_real_azure`).

---

## 8. Localizing a slowdown with `--verbose` stage timings

`analyze --verbose` emits a per-stage wall-clock breakdown to standard error, on a
channel separate from the per-object notes, so a mystery slowdown can be pinned to a
specific stage of the diagrams above without reading the code. Each line reads
`[bench-history] timing: <stage> took <elapsed>`. The stages mirror this document:

| Stage label (substring)        | Diagram location                                  |
|--------------------------------|---------------------------------------------------|
| `select_dataset (full load …)` | §2 `select_dataset` — the whole load              |
| `candidate listing + facet …`  | §3 Phase 1 listing + facet filter                 |
| `storage.list(prefix) …`       | §3 the single `storage.list` round-trip alone     |
| `git topology resolution`      | §2/§3 `resolve_history` (commit order + times)    |
| `git.first_parent ancestry …`  | §3 the first-parent ancestry walk alone (scales with history) |
| `phase 1 — key-only …`         | §3 Phase 1 filtering loop (no fetches)            |
| `phase 2/3 — concurrent fetch …` | §3 Phase 2/3 concurrent fetch + serial parse + fold |
| `series build finalization`    | §4 `SeriesBuilder::finish()` (+ serial point sort) |
| `blessing sidecar load`        | §3 history-mode blessing fetch (history only)     |
| `re-baseline blessed series`   | §2 `apply_blessings`                              |
| `change detection (find_changes …)` | §4 `find_changes_spawned` (per-series detect + FDR) |
| `report render`                | §2 `render`                                       |

The timing channel is *deliberately independent* of the per-object note stream. The
notes emit one line per stored object; at stress scale (tens of thousands of objects)
that flood would both bury the timings and distort the very wall clock being measured.
So a programmatic caller can request timings alone: `AnalyzeOptions.timing` turns on the
stage breakdown without the notes (the `--verbose` CLI flag turns on both). The stress
harness (`cargo-bench-history-stress --verbose`) uses exactly this to surface the load
breakdown while keeping its own measurement clean.

Implementation: `report::Reporter::timing(stage, elapsed)` is the sink;
`StderrReporter::with_timing(verbose, timing)` controls the two streams independently;
the stage boundaries are timed with `Instant` in `analyze_with` and `select_dataset`.
Keep the labels above in sync with the diagrams when stage boundaries move.

