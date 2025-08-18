//! Example demonstrating generic trait casting with pooled values.
//!
//! This example shows how to use the enhanced `define_pooled_dyn_cast!` macro
//! to create type-safe casting methods for traits with generic parameters.

use std::future::Future;

use blind_pool::{BlindPool, define_pooled_dyn_cast};

/// A generic trait for futures that return a specific type.
pub trait SomeFuture<T>: Future<Output = T> {}

/// Implement the trait for any type that implements Future<Output = T>
impl<F, T> SomeFuture<T> for F where F: Future<Output = T> {}

// Define the cast operation for our generic trait
define_pooled_dyn_cast!(SomeFuture<T>);

/// A trait with multiple type parameters for more complex scenarios.
pub trait Converter<Input, Output>: Send + Sync {
    /// Converts an input of type Input to an output of type Output.
    fn convert(&self, input: Input) -> Output;
}

// A concrete implementation
struct StringToIntConverter {
    _data: u8, // Add a field to give the struct non-zero size
}

impl Converter<String, i32> for StringToIntConverter {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap,
        reason = "Example code can use simplified conversion"
    )]
    fn convert(&self, input: String) -> i32 {
        input.len() as i32
    }
}

impl Converter<i32, String> for StringToIntConverter {
    fn convert(&self, input: i32) -> String {
        input.to_string()
    }
}

// Define cast for multi-parameter trait
define_pooled_dyn_cast!(Converter<Input, Output>);

// Store in a struct that uses specific trait object types
#[allow(dead_code, reason = "Example struct to demonstrate typing")]
struct ConverterCollection {
    string_to_int: blind_pool::Pooled<dyn Converter<String, i32>>,
    int_to_string: blind_pool::Pooled<dyn Converter<i32, String>>,
}

fn main() {
    println!("Demonstrating generic trait casting with pooled values");

    // Example 1: Single type parameter
    let pool = BlindPool::new();

    // Create some futures with different return types
    let future_u32 = pool.insert(async { 42_u32 });
    let future_string = pool.insert(async { "hello".to_string() });

    // Cast to trait objects with specific type parameters
    let typed_future_u32 = future_u32.cast_some_future::<u32>();
    let typed_future_string = future_string.cast_some_future::<String>();

    // Store in collections that can hold specific trait object types
    let mut int_futures: Vec<blind_pool::Pooled<dyn SomeFuture<u32>>> = Vec::new();
    let mut string_futures: Vec<blind_pool::Pooled<dyn SomeFuture<String>>> = Vec::new();

    int_futures.push(typed_future_u32);
    string_futures.push(typed_future_string);

    println!(
        "Created {} int futures and {} string futures",
        int_futures.len(),
        string_futures.len()
    );

    // Example 2: Multiple type parameters
    let converter = pool.insert(StringToIntConverter { _data: 0 });

    // Cast to trait objects with different type parameter combinations
    let string_to_int_converter = converter.clone().cast_converter::<String, i32>();
    let int_to_string_converter = converter.cast_converter::<i32, String>();

    let collection = ConverterCollection {
        string_to_int: string_to_int_converter,
        int_to_string: int_to_string_converter,
    };

    println!("Created converter collection with typed trait objects");

    // The trait objects maintain the specific type parameters
    drop(collection);
    drop(int_futures);
    drop(string_futures);

    println!("Example completed successfully!");
}
