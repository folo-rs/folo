use std::num::NonZero;

/// Informs a benchmark run callback about the basic metadata associated with the run.
///
/// This type is passed to various callback functions during benchmark execution,
/// providing information about the current thread group, total number of groups,
/// and iteration count. This allows callbacks to behave differently based on
/// their context within the benchmark run.
///
/// # Examples
///
/// ```
/// use par_bench::{Run, ThreadPool};
/// use new_zealand::nz;
///
/// # fn main() {
/// # if let Some(processors) = many_cpus::ProcessorSet::builder().take(nz!(4)) {
/// let pool = ThreadPool::new(&processors);
///
/// let run = Run::builder()
///     .groups(nz!(2)) // 2 groups of 2 threads each
///     .prepare_thread_fn(|run_meta| {
///         println!("Thread in group {} of {}",
///                  run_meta.group_index(),
///                  run_meta.group_count());
///         println!("Will execute {} iterations", run_meta.iterations());
///         
///         // Return different state based on group
///         if run_meta.group_index() == 0 {
///             "reader_thread"
///         } else {
///             "writer_thread"
///         }
///     })
///     .prepare_iter_fn(|_meta, thread_type| *thread_type)
///     .iter_fn(|thread_type: &str| {
///         match thread_type {
///             "reader_thread" => { /* reader work */ },
///             "writer_thread" => { /* writer work */ },
///             _ => unreachable!(),
///         }
///     })
///     .build();
///
/// let _results = run.execute_on(&pool, 100);
/// # }
/// # }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunMeta {
    /// The index of the current thread group, starting from 0.
    group_index: usize,

    /// The total number of thread groups in the run.
    group_count: NonZero<usize>,

    /// How many iterations will be executed as part of this run.
    iterations: u64,
}

impl RunMeta {
    /// Creates a new `RunMeta` with the specified index and total count.
    pub(crate) fn new(group_index: usize, group_count: NonZero<usize>, iterations: u64) -> Self {
        Self {
            group_index,
            group_count,
            iterations,
        }
    }

    /// The index of the current thread group, starting from 0.
    #[must_use]
    pub fn group_index(&self) -> usize {
        self.group_index
    }

    /// The total number of thread groups in the run.
    #[must_use]
    pub fn group_count(&self) -> NonZero<usize> {
        self.group_count
    }

    /// How many iterations will be executed as part of this run.
    #[must_use]
    pub fn iterations(&self) -> u64 {
        self.iterations
    }
}
