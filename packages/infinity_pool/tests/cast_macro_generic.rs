//! Tests of the `define_pooled_dyn_cast!` macro with generic traits, separate from the main crate
//! just to prove that this does not depend on accessing any private APIs.

use infinity_pool::{
    BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool, RawBlindPoolBuilder,
    RawOpaquePoolBuilder, RawPinnedPoolBuilder, define_pooled_dyn_cast,
};

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

// Define cast for generic trait to test the macro
define_pooled_dyn_cast!(TestFuture<T>);

// ========== Tests for all 8 handle types with generic trait ==========

#[test]
fn cast_managed_blind_pooled_to_generic_trait() {
    let pool = BlindPool::new();
    let async_val = AsyncValue(42_usize);
    let pooled = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled.cast_test_future::<usize>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 42);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_blind_pooled_mut_to_generic_trait() {
    let pool = BlindPool::new();
    let async_val = AsyncValue(42_usize);
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled_mut.cast_test_future::<usize>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 42);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_blind_pooled_to_generic_trait() {
    let pool = LocalBlindPool::new();
    let async_val = AsyncValue(100_i32);
    let pooled = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled.cast_test_future::<i32>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 100);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_blind_pooled_mut_to_generic_trait() {
    let pool = LocalBlindPool::new();
    let async_val = AsyncValue(100_i32);
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled_mut.cast_test_future::<i32>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 100);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_pinned_pooled_to_generic_trait() {
    let pool = PinnedPool::new();
    let async_val = AsyncValue("hello".to_string());
    let pooled = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled.cast_test_future::<String>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), "hello");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_pinned_pooled_mut_to_generic_trait() {
    let pool = PinnedPool::new();
    let async_val = AsyncValue("hello".to_string());
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled_mut.cast_test_future::<String>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), "hello");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_pinned_pooled_to_generic_trait() {
    let pool = LocalPinnedPool::new();
    let async_val = AsyncValue(250_i64);
    let pooled = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled.cast_test_future::<i64>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 250);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_pinned_pooled_mut_to_generic_trait() {
    let pool = LocalPinnedPool::new();
    let async_val = AsyncValue(250_i64);
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    let future_pooled = pooled_mut.cast_test_future::<i64>();

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    assert_eq!(future_pooled.get_output(), 250);

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(future_pooled);
    assert_eq!(pool.len(), 0);
}

// ========== Tests for raw pool removal via generic trait objects ==========

#[test]
fn cast_raw_blind_pooled_generic_removal_shared() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let async_val = AsyncValue(999_u64);
    let pooled = pool.insert(async_val).into_shared();

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled.cast_test_future::<u64>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), 999);

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_blind_pooled_generic_removal_unique() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let async_val = AsyncValue(999_u64);
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled_mut.cast_test_future::<u64>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), 999);

    // Remove via trait object
    // SAFETY: We know the handle is valid
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_opaque_pooled_generic_removal_shared() {
    let mut pool = RawOpaquePoolBuilder::new()
        .layout_of::<AsyncValue<String>>()
        .build();
    let async_val = AsyncValue("test value".to_string());
    let pooled = pool.insert(async_val).into_shared();

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled.cast_test_future::<String>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), "test value");

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_opaque_pooled_generic_removal_unique() {
    let mut pool = RawOpaquePoolBuilder::new()
        .layout_of::<AsyncValue<String>>()
        .build();
    let async_val = AsyncValue("test value".to_string());
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled_mut.cast_test_future::<String>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), "test value");

    // Remove via trait object
    // SAFETY: We know the handle is valid
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_pinned_pooled_generic_removal_shared() {
    let mut pool = RawPinnedPoolBuilder::new().build();
    let async_val = AsyncValue(vec![1, 2, 3]);
    let pooled = pool.insert(async_val).into_shared();

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled.cast_test_future::<Vec<i32>>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), vec![1, 2, 3]);

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_pinned_pooled_generic_removal_unique() {
    let mut pool = RawPinnedPoolBuilder::new().build();
    let async_val = AsyncValue(vec![1, 2, 3]);
    let pooled_mut = pool.insert(async_val);

    // Verify object is in the pool before casting
    assert_eq!(pool.len(), 1);

    // Cast to generic trait object
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let future_pooled = unsafe { pooled_mut.cast_test_future::<Vec<i32>>() };

    // Verify object is still in the pool after casting
    assert_eq!(pool.len(), 1);

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let future_ref = unsafe { future_pooled.as_ref() };
    assert_eq!(future_ref.get_output(), vec![1, 2, 3]);

    // Remove via trait object
    // SAFETY: We know the handle is valid
    unsafe {
        pool.remove(future_pooled);
    }
    assert_eq!(pool.len(), 0);
}
