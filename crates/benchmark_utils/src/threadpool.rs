use std::{
    sync::{Arc, Mutex, mpsc},
    thread::JoinHandle,
};

use many_cpus::ProcessorSet;

/// Simple minimal threadpool to allow benchmarks to run on pre-warmed threads
/// instead of creating new threads for every batch of iterations.
#[derive(Debug)]
pub struct ThreadPool {
    command_txs: Vec<mpsc::Sender<Command>>,
    join_handles: Vec<JoinHandle<()>>,
}

impl ThreadPool {
    /// Creates a threadpool with one thread per processor.
    pub fn all() -> Self {
        Self::new(ProcessorSet::all().clone())
    }

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
    /// is up to the caller to organize via sidechannels.
    pub fn enqueue_task(&self, f: impl FnOnce() + Clone + Send + 'static) {
        for tx in self.command_txs.iter() {
            tx.send(Command::Execute(Box::new(f.clone()))).unwrap();
        }
    }

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
