use std::{sync::LazyLock, thread};

use itertools::Itertools;
use nonempty::nonempty;

use crate::{pal, Processor, ProcessorSetBuilder, ProcessorSetCore};

// https://github.com/cloudhead/nonempty/issues/68
extern crate alloc;

static ALL_PROCESSORS: LazyLock<ProcessorSet> = LazyLock::new(|| {
    ProcessorSetBuilder::default()
        .take_all()
        .expect("there must be at least one processor - how could this code run if not")
});

/// One or more processors present on the system.
///
/// You can obtain the full set of processors via `ProcessorSet::all()` or specify more
/// fine-grained selection criteria via `ProcessorSet::builder()`. You can use
/// `ProcessorSet::to_builder()` to narrow down an existing set further.
///
/// One you have a `ProcessorSet`, you can iterate over `ProcessorSet::processors()`
/// to inspect the individual processors.
///
/// # Changes at runtime
///
/// It is possible that a system will have processors added or removed at runtime. This is not
/// supported - any hardware changes made at runtime will not be visible to the `ProcessorSet`
/// instances. Operations attempted on removed processors may fail with an error or panic. Added
/// processors will not be considered a member of any set.
#[derive(Clone, Debug)]
pub struct ProcessorSet {
    inner: ProcessorSetCore<pal::Platform>,
}

impl ProcessorSet {
    /// Gets a `ProcessorSet` referencing all processors on the system.
    pub fn all() -> &'static Self {
        &ALL_PROCESSORS
    }

    pub fn builder() -> ProcessorSetBuilder {
        ProcessorSetBuilder::default()
    }

    /// Returns a `ProcessorSetBuilder` that considers all processors in the current set as
    /// candidates, to be used to further narrow down the set to a specific subset.
    pub fn to_builder(&self) -> ProcessorSetBuilder {
        self.inner.to_builder().into()
    }

    fn new(inner: ProcessorSetCore<pal::Platform>) -> Self {
        Self { inner }
    }

    #[expect(clippy::len_without_is_empty)] // Never empty by definition.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn processors(&self) -> impl IntoIterator<Item = Processor> {
        // We return a converted copy of the list because it is relatively cheap to do this
        // conversion and we do not expect this to be on any hot path.
        self.inner
            .processors()
            .map(|p| Into::<Processor>::into(*p))
            .collect_vec()
    }

    /// Modifies the affinity of the current thread to execute
    /// only on the processors in this processor set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy. This behavior is not configurable.
    pub fn pin_current_thread_to(&self) {
        self.inner.pin_current_thread_to();
    }

    /// Spawns one thread for each processor in the set, pinned to that processor,
    /// providing the target processor information to the thread entry point.
    pub fn spawn_threads<E, R>(&self, entrypoint: E) -> Box<[thread::JoinHandle<R>]>
    where
        E: Fn(Processor) -> R + Send + Clone + 'static,
        R: Send + 'static,
    {
        self.inner.spawn_threads(move |p| entrypoint(p.into()))
    }

    /// Spawns a thread pinned to all processors in the set.
    ///
    /// # Behavior with multiple processors
    ///
    /// If multiple processors are present in the processor set, they might not be evenly used.
    /// An arbitrary processor may be preferentially used, with others used only when the preferred
    /// processor is otherwise busy. This behavior is not configurable.
    pub fn spawn_thread<E, R>(&self, entrypoint: E) -> thread::JoinHandle<R>
    where
        E: Fn(ProcessorSet) -> R + Send + 'static,
        R: Send + 'static,
    {
        self.inner.spawn_thread(move |p| entrypoint(p.into()))
    }
}

impl From<Processor> for ProcessorSet {
    fn from(value: Processor) -> Self {
        ProcessorSetCore::new(nonempty![value.into()], &pal::Platform).into()
    }
}

impl From<ProcessorSetCore<pal::Platform>> for ProcessorSet {
    fn from(value: ProcessorSetCore<pal::Platform>) -> Self {
        Self::new(value)
    }
}
