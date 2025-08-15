//! Integration tests for the `blind_pool` package.
//!
//! These tests verify the correctness of `BlindPool` and `Pooled<T>`
//! including thread safety, automatic cleanup, and various edge cases.

use std::sync::Arc;
use std::thread;

use blind_pool::{BlindPool, RawBlindPool};

#[test]
fn simple_insert_and_access() {
    let pool = BlindPool::from(RawBlindPool::new());

    let u32_handle = pool.insert(42_u32);
    let string_handle = pool.insert("hello".to_string());

    // Test dereferencing
    assert_eq!(*u32_handle, 42);
    assert_eq!(*string_handle, "hello");

    // Test len
    assert_eq!(pool.len(), 2);
    assert!(!pool.is_empty());
}

#[test]
fn clone_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let cloned_handle = value_handle.clone();

    // Both handles refer to the same value
    assert_eq!(*value_handle, 42);
    assert_eq!(*cloned_handle, 42);

    // Pool still has one item (not two)
    assert_eq!(pool.len(), 1);
}

#[test]
fn automatic_cleanup_single_handle() {
    let pool = BlindPool::from(RawBlindPool::new());

    {
        let _value_handle = pool.insert(42_u64);
        assert_eq!(pool.len(), 1);
    } // value_handle is dropped here

    // Value should be automatically removed
    assert_eq!(pool.len(), 0);
    assert!(pool.is_empty());
}

#[test]
fn automatic_cleanup_multiple_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle1 = pool.insert(42_u64);
    let value_handle2 = value_handle1.clone();

    assert_eq!(pool.len(), 1);

    // Drop first handle
    drop(value_handle1);
    assert_eq!(pool.len(), 1); // Still alive because of second handle

    // Drop second handle
    drop(value_handle2);
    assert_eq!(pool.len(), 0); // Now removed
}

#[test]
fn ptr_access() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let ptr = value_handle.ptr();

    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    assert_eq!(value, 42);
}

#[test]
fn erase_type_information() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let erased = value_handle.erase();

    // Can still access via cast
    // SAFETY: We know this contains a u64.
    let value = unsafe { erased.ptr().cast::<u64>().read() };
    assert_eq!(value, 42);

    // Automatic cleanup should still work
    drop(erased);
    assert_eq!(pool.len(), 0);
}

#[test]
#[should_panic(expected = "cannot erase Pooled with multiple references")]
fn erase_with_multiple_references_panics() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let _cloned_handle = value_handle.clone();

    // This should panic because there are multiple references
    let _erased = value_handle.erase();
}

#[test]
fn different_types_same_pool() {
    let pool = BlindPool::from(RawBlindPool::new());

    let u32_handle = pool.insert(42_u32);
    let f64_handle = pool.insert(2.5_f64);
    let string_handle = pool.insert("test".to_string());

    assert_eq!(pool.len(), 3);

    assert_eq!(*u32_handle, 42);
    assert!(((*f64_handle) - 2.5).abs() < f64::EPSILON);
    assert_eq!(*string_handle, "test");
}

#[test]
fn thread_safety_basic() {
    let pool = BlindPool::from(RawBlindPool::new());

    let handle = thread::spawn(move || {
        let item_handle = pool.insert(42_u64);
        *item_handle
    });

    let value = handle.join().unwrap();
    assert_eq!(value, 42);
}

#[test]
fn thread_safety_shared_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(Arc::new(42_u64));
    let cloned_handle = value_handle.clone();

    let handle = thread::spawn(move || **cloned_handle);

    let value = handle.join().unwrap();
    assert_eq!(value, 42);

    // Original handle should still work
    assert_eq!(**value_handle, 42);
}

#[test]
fn drop_with_types_that_have_drop() {
    let pool = BlindPool::from(RawBlindPool::new());

    // Test with String and Vec - types that implement Drop
    let string_handle = pool.insert("hello".to_string());
    let vec_handle = pool.insert(vec![1, 2, 3, 4, 5]);

    assert_eq!(pool.len(), 2);
    assert_eq!(*string_handle, "hello");
    assert_eq!(*vec_handle, vec![1, 2, 3, 4, 5]);

    drop(string_handle);
    drop(vec_handle);

    assert_eq!(pool.len(), 0);
}

#[test]
fn clone_pool_handles() {
    let pool1 = BlindPool::from(RawBlindPool::new());
    let pool2 = pool1.clone();

    // Both pools refer to the same underlying pool
    let value_handle1 = pool1.insert(42_u32);
    assert_eq!(pool2.len(), 1);

    let value_handle2 = pool2.insert(43_u32);
    assert_eq!(pool1.len(), 2);

    // Values are accessible from both pool handles
    assert_eq!(*value_handle1, 42);
    assert_eq!(*value_handle2, 43);
}

#[test]
fn works_with_single_byte_type() {
    let pool = BlindPool::from(RawBlindPool::new());

    let byte_handle = pool.insert(42_u8);
    assert_eq!(*byte_handle, 42);
    assert_eq!(pool.len(), 1);

    drop(byte_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
fn string_methods_through_deref() {
    let pool = BlindPool::from(RawBlindPool::new());

    let string_handle = pool.insert("hello world".to_string());

    // Test that we can call String methods directly
    assert_eq!(string_handle.len(), 11);
    assert!(string_handle.starts_with("hello"));
    assert!(string_handle.ends_with("world"));
    assert_eq!(string_handle.chars().count(), 11);
}

#[test]
fn large_number_of_items() {
    let pool = BlindPool::from(RawBlindPool::new());

    let mut handles = Vec::new();

    // Insert many items
    for i in 0..1000 {
        handles.push(pool.insert(i));
    }

    assert_eq!(pool.len(), 1000);

    // Verify values
    for (i, handle) in handles.iter().enumerate() {
        assert_eq!(**handle, i);
    }

    // Drop all handles
    handles.clear();
    assert_eq!(pool.len(), 0);
}
