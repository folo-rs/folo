# vicinal

Processor-local worker pool that schedules tasks in the vicinity of the caller.

This crate provides a worker pool where each task is executed on the same processor that
spawned it, ensuring optimal cache locality and minimizing cross-processor data movement.

## Example

```rust
use vicinal::Pool;

#[tokio::main]
async fn main() {
    let pool = Pool::new();
    let scheduler = pool.scheduler();

    let task1 = scheduler.spawn(|| 42);
    let _task2 = scheduler.spawn(|| println!("doing some stuff"));

    assert_eq!(task1.await, 42);

    let scheduler2 = scheduler.clone();
    let task3 = scheduler.spawn(move || scheduler2.spawn(|| 55));

    assert_eq!(task3.await.await, 55);
}
```

## Tradeoffs

- **Single task latency on an idle pool** is prioritized. The expectation is that tasks are
  short-lived so that the pool is often idle.

## Platform support

The package is tested on the following operating systems:

* Windows 11 x64
* Windows Server 2022 x64
* Ubuntu 24.04 x64

On non-Windows non-Linux platforms (e.g. mac OS), the package will not uphold the processor
locality guarantees, but will otherwise function correctly as a worker pool.

More details in the [package documentation](https://docs.rs/vicinal/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
