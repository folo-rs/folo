# vicinal

Processor-local worker pool that schedules tasks in the vicinity of the caller.

This crate provides a worker pool where each task is executed on the same processor that spawned
it, ensuring optimal cache locality and minimizing cross-processor data movement.

## Quick start

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

## Key features

- **Processor locality**: Tasks execute on the same processor that called `spawn()`.
- **Urgent tasks**: Use `spawn_urgent()` for high-priority tasks that execute before regular tasks.
- **Resource reuse**: Internal pooling minimizes memory allocation overhead.
- **Lazy initialization**: Per-processor resources are only allocated when first used.

## Shutdown behavior

When the `Pool` is dropped, it signals all worker threads to shut down and waits for any
currently-executing tasks to complete. Queued tasks that have not started execution are abandoned.

If you need to ensure all spawned tasks complete before shutdown, await their `JoinHandle`s
before dropping the pool.

## Panics

If a task panics, the panic is captured and re-thrown when the `JoinHandle` is awaited.
If the `JoinHandle` is dropped without being awaited, the panic is logged and discarded.
