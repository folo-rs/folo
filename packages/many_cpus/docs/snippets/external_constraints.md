# External constraints

The operating system may define constraints that prohibit the application from using all
the available processors (e.g. when the app is containerized and provided limited
hardware resources).

This package treats platform constraints as follows:

* Hard limits on which processors are allowed are respected - forbidden processors are mostly
  ignored by this package and cannot be used to spawn threads, though such processors are still
  accounted for when inspecting hardware information such as "max processor ID".
  The mechanisms for defining such limits are cgroups on Linux and job objects on Windows.
  See `examples/obey_job_affinity_limits_windows.rs` for a Windows-specific example.
* Soft limits on which processors are allowed are ignored by default - specifying a processor
  affinity via `taskset` on Linux, `start.exe /affinity 0xff` on Windows or similar mechanisms
  does not affect the set of processors this package will use by default, though you can opt in to
  this via [`.where_available_for_current_thread()`][crate::ProcessorSetBuilder::where_available_for_current_thread].
* Limits on processor time are considered an upper bound on the number of processors that can be
  included in a processor set. For example, if you configure a processor time limit of
  10 seconds per second of real time on a 20-processor system, then the builder may return up
  to 10 of the processors in the resulting processor set (though it may be a different 10 every
  time you create a new processor set from scratch). This limit is optional and may be disabled
  by using [`.ignoring_resource_quota()`][crate::ProcessorSetBuilder::ignoring_resource_quota].
  See `examples/obey_job_resource_quota_limits_windows.rs` for a Windows-specific example.

# Working with processor time constraints

If a process exceeds the processor time limit, the operating system will delay executing the
process further until the "debt is paid off". This is undesirable for most workloads because:

1. There will be random latency spikes from when the operating system decides to apply a delay.
1. The delay may not be evenly applied across all threads of the process, leading to unbalanced
   load between worker threads.

For predictable behavior that does not suffer from delay side-effects, it is important that the
process does not exceed the processor time limit. To keep out of trouble,
follow these guidelines:

* Ensure that all your concurrently executing thread pools are derived from the same processor
  set, so there is a single set of processors (up to the resource quota) that all work of the
  process will be executed on. Any new processor sets you create should be subsets of this set,
  thereby ensuring that all worker threads combined do not exceed the quota.
* Ensure that the original processor set is constructed while obeying the resource quota (which is
  enabled by default),

If your resource constraints are already applied on process startup, you can use
`SystemHardware::current().processors()` as the master set from which all other
processor sets are derived using either `.take()` or `.to_builder()`. This will ensure the
processor time quota is obeyed because `processors()` is size-limited to the quota.

```rust
# use many_cpus::SystemHardware;
# use new_zealand::nz;
let hw = SystemHardware::current();

// By taking both senders and receivers from the same original processor set, we
// guarantee that all worker threads combined cannot exceed the processor time quota.
let mail_senders = hw.processors()
    .take(nz!(2))
    .expect("need at least 2 processors for mail workers")
    .spawn_threads(|_| send_mail());

let mail_receivers = hw.processors()
    .take(nz!(2))
    .expect("need at least 2 processors for mail workers")
    .spawn_threads(|_| receive_mail());
# fn send_mail() {}
# fn receive_mail() {}
```