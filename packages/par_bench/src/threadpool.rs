use std::num::NonZero;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::{iter, mem};

use many_cpus::ProcessorSet;

/// Simple thread pool to allow benchmarks to run on pre-warmed threads instead of creating new
/// threads for every batch of iterations. Thread reuse reduces benchmark harness overhead.
///
/// Threads in the pool are bound to specific processors according to the provided processor set,
/// ensuring consistent processor affinity across benchmark runs.
///
/// # Examples
///
/// ```
/// use many_cpus::ProcessorSet;
/// use new_zealand::nz;
/// use par_bench::ThreadPool;
///
/// // Create a thread pool using the default processor set
/// let pool = ThreadPool::new(&ProcessorSet::default());
/// println!("Default pool has {} threads", pool.thread_count());
///
/// // Create a thread pool with a specific processor set
/// if let Some(processors) = ProcessorSet::builder().take(nz!(2)) {
///     let pool = ThreadPool::new(&processors);
///     assert_eq!(pool.thread_count().get(), 2);
/// }
/// ```
///
/// # Lifecycle
///
/// Dropping the pool will wait for all threads to finish executing their tasks.
#[derive(Debug)]
pub struct ThreadPool {
    command_txs: Vec<mpsc::Sender<Command>>,
    join_handles: Vec<JoinHandle<()>>,
    thread_count: NonZero<usize>,
}

impl ThreadPool {
    /// Creates a thread pool with one thread per processor in the provided processor set.
    ///
    /// Each thread will be bound to its corresponding processor, ensuring consistent
    /// processor affinity for benchmark execution.
    ///
    /// # Examples
    ///
    /// ```
    /// use many_cpus::ProcessorSet;
    /// use new_zealand::nz;
    /// use par_bench::ThreadPool;
    ///
    /// // Create pool with specific processors
    /// if let Some(processors) = ProcessorSet::builder().take(nz!(4)) {
    ///     let pool = ThreadPool::new(&processors);
    ///     assert_eq!(pool.thread_count().get(), 4);
    /// }
    /// ```
    #[must_use]
    pub fn new(processors: &ProcessorSet) -> Self {
        let (txs, rxs): (Vec<_>, Vec<_>) = iter::repeat_with(mpsc::channel)
            .take(processors.len())
            .unzip();

        let rxs = Arc::new(Mutex::new(rxs));

        let join_handles = processors
            .spawn_threads({
                let rxs = Arc::clone(&rxs);
                move |_| {
                    let rx = rxs.lock().unwrap().pop().unwrap();
                    worker_entrypoint(&rx);
                }
            })
            .into_vec();

        Self {
            thread_count: NonZero::new(txs.len())
                .expect("guarded by fact that ProcessorSet is never empty"),
            command_txs: txs,
            join_handles,
        }
    }

    /// Returns the number of threads in the pool.
    ///
    /// This is always equal to the number of processors in the processor set
    /// used to create the pool.
    ///
    /// # Examples
    ///
    /// ```
    /// use many_cpus::ProcessorSet;
    /// use par_bench::ThreadPool;
    ///
    /// let pool = ThreadPool::new(&ProcessorSet::default());
    /// println!("Pool has {} worker threads", pool.thread_count());
    /// ```
    #[must_use]
    pub fn thread_count(&self) -> NonZero<usize> {
        self.thread_count
    }

    /// Executes a task on all threads in the pool, waiting for all threads to complete
    /// and returning a collection of results.
    #[cfg_attr(test, mutants::skip)] // If work does not get enqueued, deadlocks are very easy.
    #[expect(
        clippy::needless_pass_by_ref_mut,
        reason = "protects users from deadlock through concurrent usage"
    )]
    pub(crate) fn execute_task<'f, F, R>(&mut self, f: F) -> Box<[R]>
    where
        F: FnOnce() -> R + Clone + Send + 'f,
        R: Send + 'static,
    {
        // This requires a `&mut` exclusive reference because two concurrent usages of the same
        // benchmarking thread pool are essentially guaranteed to deadlock. Internally, we have
        // no need for a `&mut` reference, this is just for caller safety.

        let mut results = Vec::with_capacity(self.thread_count.get());

        let (mut result_txs, result_rxs): (Vec<_>, Vec<_>) =
            iter::repeat_with(oneshot::channel::<R>)
                .take(self.thread_count.get())
                .unzip();

        for tx in &self.command_txs {
            // Since we guarantee that we wait for all the work to complete, the `F` does not actually
            // have to be 'static - the type system just requires that because Rust has no
            // compiler-enforced way to guarantee we wait for the work to complete.
            //
            // Therefore, we pretend it is 'static here. From the caller's point of view, they still
            // see everything with "their" lifetimes, this 'static is purely to badger Rust into
            // doing what we want.
            let f: Box<dyn FnOnce() -> R + Send + 'f> = Box::new(f.clone());

            // SAFETY: This is valid because functionally it is still 'f because we wait for the
            // callback to complete before returning from this function, so anything borrowed must
            // still be borrowed, and the callee still things they are operating under 'f lifetime.
            let f = unsafe {
                // Wololo.
                mem::transmute::<
                    Box<dyn FnOnce() -> R + Send + 'f>,
                    Box<dyn FnOnce() -> R + Send + 'static>,
                >(f)
            };

            tx.send(Command::Execute(Box::new({
                let result_tx = result_txs
                    .pop()
                    .expect("type invariant - one command_tx per thread");

                move || {
                    let result = f();

                    result_tx.send(result).expect(
                        "receiver must still exist - this is mandatory for scoped lifetime logic",
                    );
                }
            })))
            .expect("worker thread must still exist - thread pool cannot operate without workers");
        }

        for rx in result_rxs {
            results.push(
                rx.recv()
                    .expect("worker thread failed to send result - did it panic?"),
            );
        }

        results.into_boxed_slice()
    }
}

impl Drop for ThreadPool {
    #[cfg_attr(test, mutants::skip)] // Impractical to test that stuff stops happening.
    fn drop(&mut self) {
        if thread::panicking() {
            // If the thread is panicking, we are probably in a dirty state and shutting down
            // may make the problem worse by hiding the original panic, so just do nothing.
            return;
        }

        for tx in self.command_txs.drain(..) {
            tx.send(Command::Shutdown).unwrap();
        }

        for handle in self.join_handles.drain(..) {
            handle.join().unwrap();
        }
    }
}

enum Command {
    Execute(Box<dyn FnOnce() + Send>),
    Shutdown,
}

#[cfg_attr(test, mutants::skip)] // Impractical to test that things do not happen when worker function is missing.
fn worker_entrypoint(rx: &mpsc::Receiver<Command>) {
    while let Command::Execute(f) = rx.recv().unwrap() {
        f();
    }
}

#[cfg(not(miri))] // ProcessorSet is not supported under Miri.
#[cfg(test)]
mod tests {
    use std::sync::atomic::{self, AtomicUsize};

    use new_zealand::nz;

    use super::*;

    #[test]
    fn smoke_test_all() {
        let expected_default = ProcessorSet::default();
        let expected_thread_count = expected_default.len();

        let mut pool = ThreadPool::new(&ProcessorSet::default());

        assert_eq!(pool.thread_count().get(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.execute_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }

    #[test]
    fn smoke_test_one() {
        let processor_set = ProcessorSet::builder().take(nz!(1)).unwrap();
        let expected_thread_count = processor_set.len();

        let mut pool = ThreadPool::new(&processor_set);

        assert_eq!(pool.thread_count().get(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.execute_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }
}
