//! Tests of the `define_pooled_dyn_cast!` macro, separate from the main crate
//! just to prove that this does not depend on accessing any private APIs.

use std::fmt::Display;

use infinity_pool::{
    BlindPool, LocalBlindPool, RawBlindPoolBuilder, RawOpaquePoolBuilder, define_pooled_dyn_cast,
};

// Define cast for Display trait to test the macro
define_pooled_dyn_cast!(Display);

#[test]
fn cast_blind_pooled_to_display_trait() {
    let mut pool = BlindPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");
}

#[test]
fn cast_local_blind_pooled_to_display_trait() {
    let mut pool = LocalBlindPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");
}

#[test]
fn cast_raw_pooled_to_display_trait() {
    let mut pool = RawOpaquePoolBuilder::new().layout_of::<String>().build();
    let pooled = pool.insert("Test string".to_string()).into_shared();

    // Cast to trait object - raw pooled requires unsafe
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Clean up
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(display_pooled);
    }
}

#[test]
fn cast_raw_pooled_mut_to_display_trait() {
    let mut pool = RawOpaquePoolBuilder::new().layout_of::<String>().build();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object - raw pooled requires unsafe
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Clean up
    pool.remove_mut(display_pooled);
}

#[test]
fn cast_raw_blind_pooled_to_display_trait() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let pooled = pool.insert("Test string".to_string()).into_shared();

    // Cast to trait object
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Clean up
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(display_pooled);
    }
}

#[test]
fn cast_raw_blind_pooled_mut_to_display_trait() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Clean up
    pool.remove_mut(display_pooled);
}

// Test with a generic trait
pub(crate) trait TestFuture<T>: Send + Sync {
    fn get_output(&self) -> T;
}

struct AsyncValue<T>(T);
impl<T> TestFuture<T> for AsyncValue<T>
where
    T: Clone + Send + Sync,
{
    fn get_output(&self) -> T {
        self.0.clone()
    }
}

define_pooled_dyn_cast!(TestFuture<T>);

#[test]
fn cast_to_generic_trait() {
    let mut pool = BlindPool::new();
    let async_val = AsyncValue(42_usize);
    let pooled = pool.insert(async_val);

    // Cast to generic trait object
    let future_pooled = pooled.cast_test_future::<usize>();

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 42);
}
