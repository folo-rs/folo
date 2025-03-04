# many_cpus

Working on many-processor systems (e.g. 100+ logical processors) can require you to pay extra
attention to the specifics of the hardware to make optimal use of available compute capacity
and extract the most performance out of the system.

# Why should one care?

Modern operating systems try to distribute work fairly between all processors. Typical Rust
sync and async task runtimes like Rayon and Tokio likewise try to be efficient in occupying all
processors with work, even work between processors if one risks becoming idle. This is okay but
we can do better.

Taking direct over the placement of work on specific processors can yield superior performance
by taking advantage of factors under the service author's control, which are not known to general-
purpose tasking runtimes.

Most service apps exist to process requests or execute jobs - the work being done is  related to a
small data set. We can keep the data associated with a specific (HTTP/gRPC/...) request on a single
processor to ensure optimal data locality - the data related to the request is likely to be in the
caches of that processor, speeding up all operations related to that request by avoiding expensive
memory accesses.

Even when data is shared across processors, performance differences exist between different pairs
of processors because different processors can be connected to different physical memory modules.
Access to non-cached data is optimal when that data is in the same memory region as the current
processor (i.e. on the physical memory modules directly wired to the current processor).

# How does this crate help?

The `many_cpus` crate provides mechanisms to schedule threads on specific processors and in specific
memory regions, ensuring that work assigned to those threads remains on the same hardware and that
data shared between threads is local to the same memory region, enabling you to achieve high data
locality and processor cache efficiency.

In addition to thread spawning, this crate enables app logic to observe what processor and memory
region the current thread is executing in, even if the thread is not bound to a specific processor.
This can be a building block for efficiency improvements even outside directly controlled work
scheduling.

Other crates from the Folo project build upon this hardware-awareness functionality to provide
higher-level primitives such as thread pools, work schedulers, region-local cells and more.

# Quick start: spawn threads on specific processors

The simplest scenario is when you want to start a thread on every processor
(`examples/spawn_on_all_processors.rs`):

```rust
use many_cpus::ProcessorSet;

fn main() {
    let all_threads = ProcessorSet::all().spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
```

# Quick start: define processor selection criteria

Depending on the specific circumstances, you may want to filter the set of processors. For example,
you may want to use only two processors but ensure that they are high-performance processors (as
opposed to power-efficient slow processors) that are connected to the same physical memory modules
so they can cooperatively perform some processing on a shared data set
(`examples/spawn_on_selected_processors.rs`):

```rust
use std::num::NonZero;

use many_cpus::ProcessorSet;

const PROCESSOR_COUNT: NonZero<usize> = NonZero::new(2).unwrap();

fn main() {
    let selected_processors = ProcessorSet::builder()
        .same_memory_region()
        .performance_processors_only()
        .take(PROCESSOR_COUNT)
        .expect("could not find required number of processors that match the selection criteria");

    let all_threads = selected_processors.spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished.");
}
```

# Quick start: inspect the hardware environment

Functions are provided to easily inspect the current hardware environment
(`examples/observe_processor.rs`):

```rust
use std::{thread, time::Duration};

fn main() {
    loop {
        let current_processor_id = many_cpus::current_processor_id();
        let current_memory_region_id = many_cpus::current_memory_region_id();

        println!(
            "Thread executing on processor {current_processor_id} in memory region {current_memory_region_id}"
        );

        thread::sleep(Duration::from_secs(1));
    }
}
```

Note that the current processor may change at any time if you are not using threads pinned to
specific processors (such as those spawned via `ProcessorSet::spawn_threads()`). Example output:

```text
Thread executing on processor 4 in memory region 0
Thread executing on processor 4 in memory region 0
Thread executing on processor 12 in memory region 0
Thread executing on processor 2 in memory region 0
Thread executing on processor 12 in memory region 0
Thread executing on processor 0 in memory region 0
Thread executing on processor 4 in memory region 0
Thread executing on processor 4 in memory region 0
```

# Dynamic changes in hardware configuration

The crate does not support detecting and responding to changes in the hardware environment at
runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
hardware resources or OS configuration) then this change may not be visible to the crate.

# External constraints

The platform may define constraints that prohibit the application from using all the available
processors (e.g. when the app is containerized and provided limited hardware resources).

This crate treats platform constraints as follows:

* Hard limits on which processors are allowed are respected - forbidden processors are invisible to
  this crate. The mechanisms for this are `cgroups` on Linux and job objects on Windows.
* Soft limits on which processors are allowed are ignored by default - specifying a processor
  affinity via `taskset` on Linux, `start.exe /affinity 0xff` on Windows or similar mechanisms
  does not affect the set of processors this crate will use.

Note that limits on how much processor **time** may be used are ignored - they are still enforced
by the platform (which will force idle periods to keep the process within the limits) but have no
effect on which actual processors are available for use. For example, if you configure a processor
time limit of 10 seconds per second on a 20-processor system, then at full utilization this crate
would use all 20 processors but for a maximum of 0.5 seconds of each second (leaving them idle for
the other 0.5 seconds).

# Example: inheriting soft limits on allowed processors

While the crate does not by default obey soft limits, you can opt in to these limits by inheriting
the allowed processor set in the `main()` entrypoint thread, which defaults to the soft limits set
by the platform (`examples/spawn_on_inherited_processors.rs`):

```rust
use std::{thread, time::Duration};

use many_cpus::ProcessorSet;

fn main() {
    // The set of processors used here can be adjusted via platform commands.
    //
    // For example, to select only processors 0 and 1:
    // Linux: taskset 0x3 target/debug/examples/spawn_on_inherited_processors
    // Windows: start /affinity 0x3 target/debug/examples/spawn_on_inherited_processors.exe

    let inherited_processors = ProcessorSet::builder()
        .where_available_for_current_thread()
        .take_all()
        .expect("found no processors usable by the current thread - impossible because the thread is currently running on one");

    println!(
        "The current thread has inherited an affinity to {} processors.",
        inherited_processors.len()
    );

    let all_threads = inherited_processors.spawn_threads(|processor| {
        println!("Spawned thread on processor {}", processor.id());

        // In a real service, you would start some work handler here, e.g. to read
        // and process messages from a channel or to spawn a web handler.
    });

    for thread in all_threads {
        thread.join().unwrap();
    }

    println!("All threads have finished. Exiting in 10 seconds.");

    // Give some time to exit, as on Windows using "start" will create a new window that would
    // otherwise disappear instantly, making it hard to see what happened.
    thread::sleep(Duration::from_secs(10));
}
```