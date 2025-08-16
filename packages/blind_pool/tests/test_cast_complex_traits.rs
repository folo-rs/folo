//! Example demonstrating how to handle trait objects with complex trait signatures.
//!
//! Due to macro and type system limitations, complex traits require casting to a
//! trait alias that hides the specific trait signature.
//!
//! For example, you cannot define a dyn cast for something nontrivial like
//! `Future<Output = ()> + Send` but you can do it if you first define a trait alias:
//!
//! ```
//! pub trait SendUnitFuture: Future<Output = ()> + Send {}
//!
//! define_pooled_dyn_cast!(send_unit_future, SendUnitFuture);
//! ```

use std::future::Future;

use blind_pool::{BlindPool, define_pooled_dyn_cast};

/// A trait alias for futures that return unit ().
///
/// This trait makes it possible to create trait objects for futures with a specific
/// output type, which can then be stored in pooled collections.
pub trait SendUnitFuture: Future<Output = ()> + Send {}

/// Implement the trait alias for any type that implements Future<Output = ()>
impl<T> SendUnitFuture for T where T: Future<Output = ()> + Send {}

// Define the cast operation for our trait alias
define_pooled_dyn_cast!(unit_future, SendUnitFuture);

fn check_is_send_unit_future(_f: &dyn SendUnitFuture) {}

#[test]
fn future_trait_alias_example() {
    let pool = BlindPool::new();
    let pooled = pool.insert(async {
        println!("Hello from async block!");
    });

    // Cast to trait object while preserving reference counting.
    let future_pooled = pooled.cast_unit_future();

    // Verify we can use it as a Future by checking the type
    check_is_send_unit_future(&*future_pooled);
}

#[test]
fn store_futures_in_struct() {
    #[allow(dead_code, reason = "Test struct to verify compilation")]
    struct TaskManager {
        futures: Vec<blind_pool::Pooled<dyn SendUnitFuture>>,
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
