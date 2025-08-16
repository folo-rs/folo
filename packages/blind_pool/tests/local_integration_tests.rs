//! Tests for the local (single-threaded) variants of blind pool.
//!
//! These tests verify the correctness of `LocalBlindPool` and `LocalPooled<T>`
//! including basic functionality, automatic cleanup, and various edge cases.

use blind_pool::{LocalBlindPool, RawBlindPool};

#[test]
fn simple_insert_and_access() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u32_handle = pool.insert(42_u32);
    let string_handle = pool.insert("hello".to_string());

    assert_eq!(*u32_handle, 42);
    assert_eq!(*string_handle, "hello");
    assert_eq!(pool.len(), 2);
}

#[test]
fn automatic_cleanup_single_handle() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    {
        let _u32_handle = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);
    }

    // Item should be automatically removed after drop
    assert_eq!(pool.len(), 0);
    assert!(pool.is_empty());
}

#[test]
fn automatic_cleanup_multiple_handles() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u32_handle = pool.insert(42_u32);
    let cloned_handle = u32_handle.clone();

    assert_eq!(pool.len(), 1);

    // Drop first handle - item should remain
    drop(u32_handle);
    assert_eq!(pool.len(), 1);

    // Drop second handle - item should be removed
    drop(cloned_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
fn clone_handles() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let cloned_handle = value_handle.clone();

    // Both handles should refer to the same value
    assert_eq!(*value_handle, 42);
    assert_eq!(*cloned_handle, 42);

    // Modification through one handle should be visible through the other
    // (Note: we can't actually modify since we only have shared references)
    assert_eq!(*value_handle, *cloned_handle);
}

#[test]
fn clone_pool_handles() {
    let pool = LocalBlindPool::from(RawBlindPool::new());
    let pool_clone = pool.clone();

    let u32_handle = pool.insert(42_u32);
    let string_handle = pool_clone.insert("test".to_string());

    assert_eq!(*u32_handle, 42);
    assert_eq!(*string_handle, "test");
    assert_eq!(pool.len(), 2);
    assert_eq!(pool_clone.len(), 2); // Should be the same pool
}

#[test]
fn string_methods_through_deref() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let string_handle = pool.insert("hello world".to_string());

    // Test that we can call String methods directly
    assert_eq!(string_handle.len(), 11);
    assert!(string_handle.starts_with("hello"));
    assert!(string_handle.ends_with("world"));
    assert!(string_handle.contains("lo wo"));
}

#[test]
fn different_types_same_pool() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u32_handle = pool.insert(42_u32);
    let f64_handle = pool.insert(2.5_f64);
    let string_handle = pool.insert("test".to_string());

    assert_eq!(pool.len(), 3);

    assert_eq!(*u32_handle, 42);
    assert!(((*f64_handle) - 2.5).abs() < f64::EPSILON);
    assert_eq!(*string_handle, "test");
}

#[test]
fn ptr_access() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);

    // Access the value directly through dereferencing
    assert_eq!(*value_handle, 42);
}

#[test]
fn erase_type_information() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u64_handle = pool.insert(42_u64);
    let typed_clone = u64_handle.clone();
    let erased = u64_handle.erase();

    // Verify the typed handle still works
    assert_eq!(*typed_clone, 42);

    // Pool should still contain the item
    assert_eq!(pool.len(), 1);

    drop(erased);
    drop(typed_clone);
    assert_eq!(pool.len(), 0);
}

#[test]
fn erase_with_multiple_references_works() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let cloned_handle = value_handle.clone();

    // This should now work without panicking
    let erased = value_handle.erase();

    // Both handles should still work
    assert_eq!(*cloned_handle, 42);

    // Verify the erased handle is valid by ensuring cleanup works properly
    drop(erased);
    assert_eq!(*cloned_handle, 42); // Typed handle should still work

    drop(cloned_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
fn drop_with_types_that_have_drop() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    // Vec has a non-trivial Drop implementation
    let vec_handle = pool.insert(vec![1, 2, 3, 4, 5]);

    assert_eq!(vec_handle.len(), 5);
    assert_eq!(pool.len(), 1);

    drop(vec_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
fn works_with_single_byte_type() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u8_handle = pool.insert(255_u8);

    assert_eq!(*u8_handle, 255);
    assert_eq!(pool.len(), 1);

    drop(u8_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
#[cfg(not(miri))] // Miri is too slow when running tests with large data sets
fn large_number_of_items() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let mut handles = Vec::new();

    // Insert 1000 items
    for i in 0..1000 {
        let handle = pool.insert(i);
        handles.push(handle);
    }

    assert_eq!(pool.len(), 1000);

    // Verify all values are correct
    for (i, handle) in handles.iter().enumerate() {
        assert_eq!(**handle, i);
    }

    // Drop all handles
    drop(handles);

    // Pool should be empty
    assert_eq!(pool.len(), 0);
}
