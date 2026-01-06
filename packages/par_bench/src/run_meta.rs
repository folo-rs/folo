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
/// use many_cpus::SystemHardware;
/// use par_bench::{Run, ThreadPool};
/// use new_zealand::nz;
///
/// # fn main() {
/// # if let Some(processors) = SystemHardware::current().processors().to_builder().take(nz!(4)) {
/// let mut pool = ThreadPool::new(&processors);
///
/// let run = Run::new()
///     .groups(nz!(2)) // 2 groups of 2 threads each
///     .prepare_thread(|args| {
///         println!("Thread in group {} of {}",
///                  args.meta().group_index(),
///                  args.meta().group_count());
///         println!("Will execute {} iterations", args.meta().iterations());
///         
///         // Return different state based on group
///         if args.meta().group_index() == 0 {
///             "reader_thread"
///         } else {
///             "writer_thread"
///         }
///     })
///     .prepare_iter(|args| *args.thread_state())
///     .iter(|mut args| {
///         let thread_type = args.take_iter_state();
///         match thread_type {
///             "reader_thread" => { /* reader work */ },
///             "writer_thread" => { /* writer work */ },
///             _ => unreachable!(),
///         }
///     });
///
/// let _results = run.execute_on(&mut pool, 100);
/// # }
/// # }
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunMeta {
    /// The index of the current thread group, starting from 0.
    group_index: usize,

    /// The total number of thread groups in the run.
    group_count: NonZero<usize>,

    /// The total number of threads used to execute the run.
    thread_count: NonZero<usize>,

    /// How many iterations will be executed as part of this run.
    iterations: u64,
}

impl RunMeta {
    /// Creates a new `RunMeta` with the specified index and total count.
    pub(crate) fn new(
        group_index: usize,
        group_count: NonZero<usize>,
        thread_count: NonZero<usize>,
        iterations: u64,
    ) -> Self {
        Self {
            group_index,
            group_count,
            thread_count,
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
    #[cfg_attr(test, mutants::skip)] // cargo-mutants does not understand NonZero - unviable.
    pub fn group_count(&self) -> NonZero<usize> {
        self.group_count
    }

    /// The total number of threads used to execute the run.
    #[must_use]
    #[cfg_attr(test, mutants::skip)] // cargo-mutants does not understand NonZero - unviable.
    pub fn thread_count(&self) -> NonZero<usize> {
        self.thread_count
    }

    /// How many iterations will be executed as part of this run.
    #[must_use]
    pub fn iterations(&self) -> u64 {
        self.iterations
    }
}
