Utilities for parsing and emitting strings in the the `cpulist` format often used by Linux
utilities that work with processor IDs, memory region IDs and similar numeric hardware
identifiers.

Example cpulist string: `0,1,2-4,5-9:2,6-10:2`

```rust
let selected_processors = cpulist::parse("0-9,32-35,40").unwrap();
assert_eq!(
    selected_processors,
    vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 32, 33, 34, 35, 40]
);

println!("Selected processors: {selected_processors:?}");
println!("As cpulist: {}", cpulist::emit(selected_processors));
```

More details in the [package documentation](https://docs.rs/cpulist/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.