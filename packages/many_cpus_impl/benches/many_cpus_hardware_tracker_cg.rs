//! Callgrind benchmarks for the `SystemHardware` per-thread tracking read path.
//!
//! Paired with `many_cpus_hardware_tracker.rs` (the Criterion group) which
//! covers the same operations under wall-clock measurement.
//!
//! These isolate the instruction-level cost of resolving the current
//! processor / memory region, which `region_cached` and `region_local`
//! perform on every cached read:
//!
//! * `current_memory_region_id_unpinned` — unpinned thread; the pin-state
//!   lookup misses (no entry for this instance).
//! * `current_memory_region_id_pinned` — pinned thread; the pin-state lookup
//!   hits the single entry stored for the `current()` singleton.
//! * `current_processor_id_pinned` — same hit path for the processor ID.

#![allow(
    missing_docs,
    reason = "no need for API documentation on benchmark code"
)]
#![cfg_attr(
    target_os = "linux",
    expect(
        clippy::exit,
        clippy::missing_docs_in_private_items,
        unused_qualifications,
        reason = "Triggered by Gungraun macro expansion. Tracking issue drafts live at \
          c:/Source/gungraun-lint-issues/ pending upstream filing."
    )
)]

#[cfg(not(target_os = "linux"))]
fn main() {
    // Gungraun requires Valgrind, which is Linux-only. On other platforms this
    // bench target compiles to a no-op so `cargo build --all-targets` still works.
}

#[cfg(target_os = "linux")]
mod linux {
    use std::hint::black_box;

    use gungraun::prelude::*;
    use many_cpus::SystemHardware;
    use new_zealand::nz;

    // `current()` hands back the process-wide singleton by `&'static` reference, so
    // there is no `Drop` to keep out of the measured region. The pinned setup pins
    // the thread before measurement so the pin-state lookup hits its single entry.
    fn unpinned_hardware() -> &'static SystemHardware {
        SystemHardware::current()
    }

    fn pinned_hardware() -> &'static SystemHardware {
        let hw = SystemHardware::current();
        hw.processors()
            .take(nz!(1))
            .expect("a single processor is always available")
            .pin_current_thread_to();
        hw
    }

    #[library_benchmark]
    #[bench::unpinned(unpinned_hardware())]
    fn current_memory_region_id_unpinned(hw: &'static SystemHardware) {
        black_box(hw.current_memory_region_id());
    }

    #[library_benchmark]
    #[bench::pinned(pinned_hardware())]
    fn current_memory_region_id_pinned(hw: &'static SystemHardware) {
        black_box(hw.current_memory_region_id());
    }

    #[library_benchmark]
    #[bench::pinned(pinned_hardware())]
    fn current_processor_id_pinned(hw: &'static SystemHardware) {
        black_box(hw.current_processor_id());
    }

    library_benchmark_group!(
        name = read_group,
        benchmarks = [
            current_memory_region_id_unpinned,
            current_memory_region_id_pinned,
            current_processor_id_pinned,
        ]
    );
}

#[cfg(target_os = "linux")]
use gungraun::{Callgrind, CallgrindMetrics, LibraryBenchmarkConfig};
#[cfg(target_os = "linux")]
pub use linux::read_group;

#[cfg(target_os = "linux")]
gungraun::main!(
    config = LibraryBenchmarkConfig::default().tool(
        Callgrind::default()
            .args(["--branch-sim=yes"])
            .format([CallgrindMetrics::Default, CallgrindMetrics::BranchSim]),
    );
    library_benchmark_groups = read_group
);
