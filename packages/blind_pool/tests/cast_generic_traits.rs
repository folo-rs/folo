//! Tests for generic trait casting functionality.

use std::future::Future;

use blind_pool::{BlindPool, LocalBlindPool, RawBlindPool, define_pooled_dyn_cast};

/// A generic trait for futures that return a specific type.
pub trait SomeFuture<T>: Future<Output = T> {}

/// Implement the trait for any type that implements Future<Output = T>
impl<F, T> SomeFuture<T> for F where F: Future<Output = T> {}

// Define the cast operation for our generic trait
define_pooled_dyn_cast!(SomeFuture<T>);

#[test]
fn pooled_cast_generic_future() {
    let pool = BlindPool::new();
    let pooled = pool.insert(async { 42_usize });

    // Cast to trait object with specific type parameter.
    let future_pooled = pooled.cast_some_future::<usize>();

    // Verify the type is correct
    drop(future_pooled);
}

#[test]
fn local_pooled_cast_generic_future() {
    let pool = LocalBlindPool::new();
    let pooled = pool.insert(async { "hello" });

    // Cast to trait object with specific type parameter.
    let future_pooled = pooled.cast_some_future::<&'static str>();

    // Verify the type is correct
    drop(future_pooled);
}

#[test]
fn raw_pooled_cast_generic_future() {
    let mut pool = RawBlindPool::new();
    let pooled = pool.insert(async { 2.71_f64 });

    // Cast to trait object with specific type parameter.
    // SAFETY: We must guarantee the item is still in the pool (yep) and
    // that no concurrent `&mut` exclusive references exist (yep).
    let future_pooled = unsafe { pooled.cast_some_future::<f64>() };

    // Verify the type is correct by using the value in type position
    let typed_future_pooled: blind_pool::RawPooled<dyn SomeFuture<f64>> = future_pooled;

    // Clean up via the post-cast handle.
    pool.remove(&typed_future_pooled);
}

#[test]
fn store_generic_futures_in_struct() {
    #[allow(dead_code, reason = "Test struct to verify compilation")]
    struct TaskManager {
        int_futures: Vec<blind_pool::Pooled<dyn SomeFuture<i32>>>,
        string_futures: Vec<blind_pool::Pooled<dyn SomeFuture<String>>>,
    }

    let pool = BlindPool::new();

    let int_task1 = pool.insert(async { 10_i32 });
    let int_task2 = pool.insert(async { 20_i32 });
    let string_task = pool.insert(async { "result".to_string() });

    let int_future1 = int_task1.cast_some_future::<i32>();
    let int_future2 = int_task2.cast_some_future::<i32>();
    let string_future = string_task.cast_some_future::<String>();

    let task_manager = TaskManager {
        int_futures: vec![int_future1, int_future2],
        string_futures: vec![string_future],
    };

    // Verify we can access the stored futures
    assert_eq!(task_manager.int_futures.len(), 2);
    assert_eq!(task_manager.string_futures.len(), 1);
    drop(task_manager);
}

/// A trait with multiple type parameters to test more complex generics.
pub trait MultiParam<T, U>: Send + Sync {
    /// Processes an input of type T and returns a value of type U.
    fn process(&self, input: T) -> U;
}

// Test struct that implements the multi-parameter trait
struct Processor {
    _phantom: u8, // Add a field to give the struct non-zero size
}

impl MultiParam<i32, String> for Processor {
    fn process(&self, input: i32) -> String {
        input.to_string()
    }
}

impl MultiParam<String, i32> for Processor {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Test code can use simplified conversion"
    )]
    fn process(&self, input: String) -> i32 {
        input.len() as i32
    }
}

// Define cast for multi-parameter trait
define_pooled_dyn_cast!(MultiParam<T, U>);

#[test]
fn multi_param_trait_cast() {
    let pool = BlindPool::new();
    let processor = pool.insert(Processor { _phantom: 0 });

    // Cast to trait object with specific type parameters.
    let processor_i32_string = processor.clone().cast_multi_param::<i32, String>();
    let processor_string_i32 = processor.cast_multi_param::<String, i32>();

    // Verify the types are correct
    drop(processor_i32_string);
    drop(processor_string_i32);
}
