# External constraints

The operating system may define constraints that prohibit the application from using all
the available processors (e.g. when the app is containerized and provided limited
hardware resources).

This crate treats platform constraints as follows:

* Hard limits on which processors are allowed are respected - forbidden processors are mostly
  ignored by this crate and cannot be used to spawn threads, though such processors are still
  accounted for when inspecting hardware information such as "max processor ID".
  The mechanisms for defining such limits are cgroups on Linux and job objects on Windows.
  See `examples/obey_job_affinity_limits_windows.rs` for a Windows-specific example.
* Soft limits on which processors are allowed are ignored by default - specifying a processor
  affinity via `taskset` on Linux, `start.exe /affinity 0xff` on Windows or similar mechanisms
  does not affect the set of processors this crate will use by default, though you can opt in to
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
`ProcessorSet::default()` as the master set from which all other processor sets are derived using
`ProcessorSet::default().to_builder()`. This will ensure the processor time quota is always obeyed
because `ProcessorSet::default()` is guaranteed to obey the resource quota.

```rust ignore
let mail_senders = ProcessorSet::default().to_builder().take(MAIL_WORKER_COUNT).unwrap();
```