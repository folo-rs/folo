use std::{sync::LazyLock, thread};

use itertools::Itertools;
use nonempty::{nonempty, NonEmpty};

use crate::{pal, Processor, ProcessorSetBuilder, ProcessorSetCore};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

static ALL_PROCESSORS: LazyLock<ProcessorSet> = LazyLock::new(|| {
    ProcessorSetBuilder::default()
        .take_all()
        .expect("there must be at least one processor - how could this code run if not")
});

// This is a specialization of the *Core type for the build target platform. It is the only
// specialization available via the crate's public API surface - other specializations
// exist only for unit testing purposes where the platform is mocked, in which case the
// *Core type is used directly instead of using a newtype wrapper like we have here.

/// One or more processors present on the system and available for use.
///
/// You can obtain the full set of available processors via [`ProcessorSet::all()`] or specify more
/// fine-grained selection criteria via [`ProcessorSet::builder()`]. You can use
/// [`ProcessorSet::to_builder()`] to further narrow down an existing set.
///
/// One you have a [`ProcessorSet`], you can iterate over [`ProcessorSet::processors()`]
/// to inspect the individual processors in the set or use [`ProcessorSet::spawn_threads()`] to
/// spawn a set of threads pinned to each of the processors in the set, one thread per processor.
/// You may also use [``ProcessorSet::spawn_thread()`] to spawn a single thread pinned to all
/// processors in the set, allowing the thread to move but only between the processors in the set.
///
/// # Changes at runtime
///
/// It is possible that a system will have processors added or removed at runtime. This is not
/// supported - any hardware changes made at runtime will not be visible to the `ProcessorSet`
/// instances. Operations attempted on removed processors may fail with an error or panic or
/// silently misbehave (e.g. threads never starting). Added processors will not be considered a
/// member of any set.
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    core: ProcessorSetCore<pal::BuildTargetPlatform>,
}

impl ProcessorSet {
    /// Gets a [`ProcessorSet`] referencing all present and available processors on the system.
    ///
    /// This set does not include processors that are not available to the current process or those
    /// that are purely theoretical (e.g. processors that may be added later to the system but are
    /// not yet present).
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    /// Creates a builder that can be used to construct a processor set with specific criteria.
    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    /// Returns a [`ProcessorSetBuilder`] that is narrowed down to all processors in the current
    /// set, to be used to further narrow down the set to a specific subset.
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        self.core.to_builder().into()
    }

    fn new(inner: ProcessorSetCore<pal::BuildTargetPlatform>) -> Self {
        Self { core: inner }
    }

    /// Returns a [`ProcessorSet`] containing the provided processors.
    pub fn from_processors(processors: NonEmpty<Processor>) -> Self {
        Self::new(ProcessorSetCore::new(
            processors.map(|p| p.core),
            &pal::BUILD_TARGET_PLATFORM,
        ))
    }

    /// Returns the number of processors in the set. A processor set is never empty.
    #[expect(clippy::len_without_is_empty)] // Never empty by definition.
    pub fn len(&self) -> usize {
        self.core.len()
    }

    /// Returns a collection containing all the processors in the set.
    pub fn processors(&self) -> NonEmpty<Processor> {
        // We return a converted copy of the list because it is relatively cheap to do this
        // conversion and we do not expect this to be on any hot path.
        NonEmpty::from_vec(
            self.core
                .processors()
                .map(|p| Into::<Processor>::into(*p))
                .collect_vec(),
        )
        .expect("processor sets contain at least one processor by definition")
    }

    /// Modifies the affinity of the current thread to execute
    /// only on the processors in this processor set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy.
    pub fn pin_current_thread_to(&self) {
        self.core.pin_current_thread_to();
    }

    /// Spawns one thread for each processor in the set, pinned to that processor,
    /// providing the target processor information to the thread entry point.
    ///
    /// Each spawned thread will only be scheduled on one of the processors in the set. When that
    /// processor is busy, the thread will simply wait for the processor to become available.
    pub fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
    where
        E: Fn(Processor) -> R + Send + Clone + 'static,
        R: Send + 'static,
    {
        self.core.spawn_threads(move |p| entrypoint(p.into()))
    }

    /// Spawns a single thread pinned to the set. The thread will only be scheduled to execute on
    /// the processors in the processor set and may freely move between the processors in the set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy.
    pub fn spawn_thread<E, R>(&self, entrypoint: E) -> thread::JoinHandle<R>
    where
        E: Fn(ProcessorSet) -> R + Send + 'static,
        R: Send + 'static,
    {
        self.core.spawn_thread(move |p| entrypoint(p.into()))
    }
}

impl From<Processor> for ProcessorSet {
    fn from(value: Processor) -> Self {
        let processor_core = *value.as_ref();
        ProcessorSetCore::new(nonempty![processor_core], &pal::BUILD_TARGET_PLATFORM).into()
    }
}

impl From<ProcessorSetCore<pal::BuildTargetPlatform>> for ProcessorSet {
    fn from(value: ProcessorSetCore<pal::BuildTargetPlatform>) -> Self {
        Self::new(value)
    }
}
