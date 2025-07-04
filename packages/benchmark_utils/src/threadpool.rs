use std::iter::repeat_with;
use std::num::NonZero;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};

use many_cpus::ProcessorSet;

/// Simple thread pool to allow benchmarks to run on pre-warmed threads instead of creating new
/// threads for every batch of iterations. Thread reuse reduces benchmark harness overhead.
///
/// # Lifecycle
///
/// Dropping the pool will wait for all threads to finish executing their tasks.
#[derive(Debug)]
pub struct ThreadPool {
    command_txs: Vec<mpsc::Sender<Command>>,
    join_handles: Vec<JoinHandle<()>>,
}

impl ThreadPool {
    /// Creates a thread pool with one thread per processor in the provided processor set.
    #[must_use]
    pub fn new(processors: &ProcessorSet) -> Self {
        let (txs, rxs): (Vec<_>, Vec<_>) =
            repeat_with(mpsc::channel).take(processors.len()).unzip();

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
            command_txs: txs,
            join_handles,
        }
    }

    /// Enqueues a task to be executed on all threads in the pool.
    ///
    /// Will not wait for the task to complete - getting back any result
    /// is up to the caller to organize via side-channels.
    #[cfg_attr(test, mutants::skip)] // If work does not get enqueued, deadlocks are very easy.
    pub fn enqueue_task(&self, f: impl FnOnce() + Clone + Send + 'static) {
        for tx in &self.command_txs {
            tx.send(Command::Execute(Box::new(f.clone()))).unwrap();
        }
    }

    /// Numbers of threads in the pool.
    #[must_use]
    pub fn thread_count(&self) -> NonZero<usize> {
        NonZero::new(self.command_txs.len())
            .expect("thread pool cannot be created with zero threads")
    }
}

impl Default for ThreadPool {
    /// Creates a thread pool with one thread per processor available to the current process.
    fn default() -> Self {
        Self::new(&ProcessorSet::default())
    }
}

impl Drop for ThreadPool {
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

#[cfg_attr(test, mutants::skip)] // Timeout due to waiting forever on a Barrier. Whatever.
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

        let pool = ThreadPool::default();

        assert_eq!(pool.thread_count().get(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.enqueue_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        // Waits for all threads to complete their work.
        drop(pool);

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }

    #[test]
    fn smoke_test_one() {
        let processor_set = ProcessorSet::builder().take(nz!(1)).unwrap();
        let expected_thread_count = processor_set.len();

        let pool = ThreadPool::new(&processor_set);

        assert_eq!(pool.thread_count().get(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.enqueue_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        // Waits for all threads to complete their work.
        drop(pool);

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }
}
