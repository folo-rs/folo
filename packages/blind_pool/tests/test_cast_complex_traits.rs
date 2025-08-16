//! Example demonstrating how to handle complex trait objects like Future.
//!
//! For traits with associated types that need specific values (like Future<Output = ()>),
//! you can create a trait alias and cast to that.

use blind_pool::{BlindPool, define_pooled_dyn_cast};
use std::future::Future;

/// A trait alias for futures that return unit ().
///
/// This trait makes it possible to create trait objects for futures with a specific
/// output type, which can then be stored in pooled collections.
pub trait UnitFuture: Future<Output = ()> {}

// Implement the trait alias for any type that implements Future<Output = ()>
impl<T> UnitFuture for T where T: Future<Output = ()> {}

// Define the cast operation for our trait alias
define_pooled_dyn_cast!(unit_future, UnitFuture);

fn check_is_unit_future(_f: &dyn UnitFuture) {}

#[test]
fn future_trait_alias_example() {
    let pool = BlindPool::new();
    let pooled = pool.insert(async {
        println!("Hello from async block!");
    });

    // Cast to trait object while preserving reference counting.
    let future_pooled = pooled.cast_unit_future();

    // Verify we can use it as a Future by checking the type
    check_is_unit_future(&*future_pooled);
}

#[test]
fn store_futures_in_struct() {
    #[allow(dead_code, reason = "Test struct to verify compilation")]
    struct TaskManager {
        futures: Vec<blind_pool::Pooled<dyn UnitFuture>>,
    }

    let pool = BlindPool::new();

    let task1 = pool.insert(async {
        println!("Task 1");
    });
    let task2 = pool.insert(async {
        println!("Task 2");
    });

    let future1 = task1.cast_unit_future();
    let future2 = task2.cast_unit_future();

    let task_manager = TaskManager {
        futures: vec![future1, future2],
    };

    // Verify we can access the stored futures
    assert_eq!(task_manager.futures.len(), 2);
    drop(task_manager);
}
