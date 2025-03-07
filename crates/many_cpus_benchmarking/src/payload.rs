/// One benchmark payload, to be processed by each worker involved in each benchmark.
///
/// Payloads are created in pairs because the workers are created in pairs. Depending on the
/// benchmark scenario, the pair of payloads may be connected (e.g. reader and writer) or
/// independent (equivalent, two workers doing the same thing).
///
/// The lifecycle of a payload is:
///
/// 1. A payload pair is created on the main thread.
/// 1. Each payload in the pair is transferred to a specific thread hosting a specific worker.
/// 1. The `prepare()` method is called to generate any input data.
/// 1. The payload pair is exchanged between the two paired workers.
/// 1. The `process()` method is called to process the data received from the other pair member.
/// 1. The payload pair is dropped.
///
/// Note that some [work distribution modes][crate::WorkDistribution] (named `*Self`) may skip
/// the payload exchange step.
pub trait Payload: Sized + Send + 'static {
    /// Creates the payload pair that will be used to initialize one worker pair in one
    /// benchmark iteration. This will be called on the main thread.
    fn new_pair() -> (Self, Self);

    /// Performs any initialization required. This will be called before the benchmark time span
    /// measurement starts. It will be called on a worker thread but the payload may be moved to
    /// a different worker thread before the benchmark starts (as workers by default prepare work
    /// for each other, to showcase what happens when the work is transferred between threads).
    fn prepare(&mut self);

    /// Processes the payload but does not consume it. The iteration is complete when this returns
    /// for all payloads. The payloads are dropped later, to ensure that the benchmark time is not
    /// affected by the time it takes to drop the payload and release the memory.
    fn process(&mut self);
}
