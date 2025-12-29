On many-processor systems with multiple memory regions, there is an extra cost associated with
accessing data in physical memory modules that are in a different memory region than the current
processor:

* Cross-memory-region loads have higher latency (e.g. 100 ns local versus 200 ns remote).
* Cross-memory-region loads have lower throughput (e.g. 50 Gbps local versus 10 Gbps remote).

This crate provides the capability to create static variables that maintain separate storage per
memory region. This may be useful in circumstances where state needs to be shared but only within
each memory region (e.g. because you intentionally want to avoid the overhead of cross-memory-region
transfers and want to isolate the data sets).

Think of this as an equivalent of `thread_local_rc!`, except operating on the memory region boundary
instead of the thread boundary.

```rust
// `RegionLocalExt` provides required extension methods on region-local
// static variables, such as `with_local()` and `set_local()`.
use region_local::{RegionLocalExt, region_local};

region_local!(static FAVORITE_COLOR: String = "blue".to_string());

FAVORITE_COLOR.with_local(|color| {
    println!("My favorite color is {color}");
});

FAVORITE_COLOR.set_local("red".to_string());
```

## See also

More details in the [package documentation](https://docs.rs/region_local/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.