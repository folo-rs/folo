use std::{
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};

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
    /// Creates a thread pool with one thread per processor available to the current process.
    pub fn all() -> Self {
        Self::new(ProcessorSet::all().clone())
    }

    /// Creates a thread pool with one thread per processor in the provided processor set.
    pub fn new(processors: ProcessorSet) -> Self {
        let (txs, rxs): (Vec<_>, Vec<_>) = (0..(processors.len())).map(|_| mpsc::channel()).unzip();

        let rxs = Arc::new(Mutex::new(rxs));

        let join_handles = processors
            .spawn_threads({
                let rxs = Arc::clone(&rxs);
                move |_| {
                    let rx = rxs.lock().unwrap().pop().unwrap();
                    worker_entrypoint(rx);
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
        for tx in self.command_txs.iter() {
            tx.send(Command::Execute(Box::new(f.clone()))).unwrap();
        }
    }

    /// Numbers of threads in the pool.
    pub fn thread_count(&self) -> usize {
        self.command_txs.len()
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
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

fn worker_entrypoint(rx: mpsc::Receiver<Command>) {
    while let Command::Execute(f) = rx.recv().unwrap() {
        f();
    }
}

#[cfg(not(miri))] // ProcessorSet is not supported under Miri.
#[cfg(test)]
mod tests {
    use std::{
        num::NonZero,
        sync::atomic::{self, AtomicUsize},
    };

    use super::*;

    #[test]
    fn smoke_test_all() {
        let expected_all = ProcessorSet::all();
        let expected_thread_count = expected_all.len();

        let pool = ThreadPool::all();

        assert_eq!(pool.thread_count(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.enqueue_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        // Waits for all thereads to complete their work.
        drop(pool);

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }

    #[test]
    fn smoke_test_one() {
        let processor_set = ProcessorSet::builder()
            .take(NonZero::new(1).unwrap())
            .unwrap();
        let expected_thread_count = processor_set.len();

        let pool = ThreadPool::new(processor_set);

        assert_eq!(pool.thread_count(), expected_thread_count);

        let counter = Arc::new(AtomicUsize::new(0));

        pool.enqueue_task({
            let counter = Arc::clone(&counter);
            move || {
                counter.fetch_add(1, atomic::Ordering::SeqCst);
            }
        });

        // Waits for all thereads to complete their work.
        drop(pool);

        assert_eq!(
            counter.load(atomic::Ordering::SeqCst),
            expected_thread_count
        );
    }
}
