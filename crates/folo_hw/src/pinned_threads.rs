//! Easily spawn threads pinned to specific processors (CPU cores).

use std::{
    num::{NonZero, NonZeroU32, NonZeroUsize},
    thread::{self, JoinHandle, Thread},
};

use windows::Win32::System::{
    SystemInformation::GROUP_AFFINITY,
    Threading::{
        GetActiveProcessorCount, GetActiveProcessorGroupCount, GetCurrentThread,
        SetThreadGroupAffinity,
    },
};

use crate::{hardware::get_active_processors_by_memory_region, MemoryRegionId, ProcessorId};

/// How many threads to spawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Instantiation {
    /// Spawns one thread for every processor.
    ///
    /// Only considers processors that the current process is permitted to use.
    PerProcessor,

    /// Spawns one thread for every memory region (NUMA node).
    ///
    /// Only considers processors that the current process is permitted to use.
    PerMemoryRegion,
}

/// What exactly at thread is pinned to in terms of hardware resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pinning {
    /// Each thread is pinned to only execute on a specific processor.
    /// If that processor is busy, the thread will simply wait.
    PerProcessor,
}

/// Provides some potentially useful context to a pinned thread's entry point.
#[derive(Debug)]
pub struct PinnedThreadContext {
    // TODO: There are models where we might not pin to one specific processor but multiple.
    processor: ProcessorId,
    memory_region: MemoryRegionId,
}

impl PinnedThreadContext {
    /// The processor that this thread is pinned to.
    pub fn processor(&self) -> &ProcessorId {
        &self.processor
    }

    /// The memory region that this thread is pinned to.
    pub fn memory_region(&self) -> &MemoryRegionId {
        &self.memory_region
    }
}

/// Spawns a number of spawned threads, returning join handles that
/// can be used to wait for them to finish.
///
/// Dropping the join handles has no impact on the spawned threads.
pub fn spawn<E, R>(
    instantiation: Instantiation,
    pinning: Pinning,
    entrypoint: E,
) -> Box<[JoinHandle<R>]>
where
    E: Fn(PinnedThreadContext) -> R + Send + Clone + 'static,
    R: Send + 'static,
{
    match instantiation {
        Instantiation::PerProcessor => spawn_per_processor(entrypoint),
        Instantiation::PerMemoryRegion => spawn_per_memory_region(entrypoint),
    }
}

fn spawn_per_processor<E, R>(entrypoint: E) -> Box<[JoinHandle<R>]>
where
    E: Fn(PinnedThreadContext) -> R + Send + Clone + 'static,
    R: Send + 'static,
{
    let processors = get_active_processors_by_memory_region();

    let mut join_handles =
        Vec::with_capacity(processors.iter().map(|r| r.processors().len()).sum());

    for region in processors {
        for processor in region.processors() {
            let context = PinnedThreadContext {
                memory_region: *region.id(),
                processor: *processor,
            };

            let entrypoint = entrypoint.clone();
            join_handles.push(thread::spawn(move || {
                pin_current_thread_to_processor(context.processor);

                entrypoint(context)
            }));
        }
    }

    join_handles.into_boxed_slice()
}

fn spawn_per_memory_region<E, R>(entrypoint: E) -> Box<[JoinHandle<R>]>
where
    E: Fn(PinnedThreadContext) -> R + Send + Clone + 'static,
    R: Send + 'static,
{
    let processors = get_active_processors_by_memory_region();

    let mut join_handles =
        Vec::with_capacity(processors.iter().map(|r| r.processors().len()).sum());

    for region in processors {
        let processor = region
            .processors()
            .first()
            .expect("expecting to have at least one processor in every active memory region");

        let context = PinnedThreadContext {
            memory_region: *region.id(),
            processor: *processor,
        };

        let entrypoint = entrypoint.clone();
        join_handles.push(thread::spawn(move || {
            pin_current_thread_to_processor(context.processor);

            entrypoint(context)
        }));
    }

    join_handles.into_boxed_slice()
}

fn pin_current_thread_to_processor(id: ProcessorId) {
    // This is a pseudo handle and does not need to be closed.
    // SAFETY: Nothing required, just an FFI call.
    let current_thread = unsafe { GetCurrentThread() };

    let group_affinity = GROUP_AFFINITY {
        Group: id.group(),
        Mask: 1 << id.index_in_group(),
        ..Default::default()
    };

    unsafe { SetThreadGroupAffinity(current_thread, &group_affinity, None) }.unwrap();
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn smoke_test() {
        let handles = spawn(Instantiation::PerProcessor, Pinning::PerProcessor, |ctx| {
            println!("{ctx:?}");

            if ctx.processor().index_in_group() % 2 == 0 {
                // Burn a lot of CPU.
                loop {
                    std::hint::spin_loop();
                }
            }

            thread::sleep(Duration::from_secs(100));
        });

        for handle in handles {
            handle.join().unwrap();
        }
    }
}
