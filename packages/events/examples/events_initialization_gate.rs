//! Demonstrates using [`ManualResetEvent`] as an initialization gate.
//!
//! A shared resource must be initialized before workers can use it.
//! Workers start immediately but wait on the gate; once initialization
//! completes, all workers are released at once and future workers pass
//! through without blocking.

use std::sync::{Arc, Mutex};

use events::ManualResetEvent;

const WORKER_COUNT: usize = 5;

#[tokio::main]
async fn main() {
    initialization_gate().await;
    pause_resume().await;
}

/// Workers wait for a shared resource to be initialized before proceeding.
async fn initialization_gate() {
    let gate = ManualResetEvent::boxed();
    let config: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));

    // Spawn workers that depend on the configuration.
    let mut handles = Vec::new();
    for id in 0..WORKER_COUNT {
        let gate = gate.clone();
        let config = Arc::clone(&config);
        handles.push(tokio::spawn(async move {
            // Wait until initialization is done.
            gate.wait().await;

            let value = config
                .lock()
                .expect("not poisoned")
                .clone()
                .expect("config is set before gate opens");
            println!("Worker {id} sees config: {value}");
        }));
    }

    // Simulate initialization.
    *config.lock().expect("not poisoned") = Some("database_url=localhost:5432".to_string());

    // Open the gate — all workers proceed.
    gate.set();

    for handle in handles {
        handle.await.expect("worker did not panic");
    }

    println!("All workers completed.");
}

/// A control task can pause and resume processing by resetting and
/// setting a [`ManualResetEvent`].
async fn pause_resume() {
    let gate = ManualResetEvent::boxed();
    let counter: Arc<Mutex<u32>> = Arc::new(Mutex::new(0));

    // Start in the "running" state.
    gate.set();

    let worker_gate = gate.clone();
    let worker_counter = Arc::clone(&counter);
    let worker = tokio::spawn(async move {
        const ROUNDS: u32 = 6;
        for _ in 0..ROUNDS {
            // Each round, wait for the gate to be open.
            worker_gate.wait().await;
            let mut c = worker_counter.lock().expect("not poisoned");
            *c = c.checked_add(1).expect("counter fits in u32");
        }
    });

    // Let the worker run for a bit, then pause.
    tokio::task::yield_now().await;
    gate.reset();
    println!(
        "Paused. Counter so far: {}",
        counter.lock().expect("not poisoned")
    );

    // Resume — the worker continues from where it left off.
    gate.set();
    worker.await.expect("worker did not panic");

    println!(
        "Resumed and finished. Final counter: {}",
        counter.lock().expect("not poisoned")
    );
}
