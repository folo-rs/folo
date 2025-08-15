//! Integration tests for the `blind_pool_managed` package.
//!
//! These tests verify the correctness of `ManagedBlindPool` and `ManagedPooled<T>`
//! including thread safety, automatic cleanup, and various edge cases.

use std::sync::Arc;
use std::thread;

use blind_pool::{BlindPool, RawBlindPool};

#[test]
fn simple_insert_and_access() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_u32 = pool.insert(42_u32);
    let managed_string = pool.insert("hello".to_string());

    // Test dereferencing
    assert_eq!(*managed_u32, 42);
    assert_eq!(*managed_string, "hello");

    // Test len
    assert_eq!(pool.len(), 2);
    assert!(!pool.is_empty());
}

#[test]
fn clone_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_value = pool.insert(42_u64);
    let cloned_handle = managed_value.clone();

    // Both handles refer to the same value
    assert_eq!(*managed_value, 42);
    assert_eq!(*cloned_handle, 42);

    // Pool still has one item (not two)
    assert_eq!(pool.len(), 1);
}

#[test]
fn automatic_cleanup_single_handle() {
    let pool = BlindPool::from(RawBlindPool::new());

    {
        let _managed_value = pool.insert(42_u64);
        assert_eq!(pool.len(), 1);
    } // managed_value is dropped here

    // Value should be automatically removed
    assert_eq!(pool.len(), 0);
    assert!(pool.is_empty());
}

#[test]
fn automatic_cleanup_multiple_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_value1 = pool.insert(42_u64);
    let managed_value2 = managed_value1.clone();

    assert_eq!(pool.len(), 1);

    // Drop first handle
    drop(managed_value1);
    assert_eq!(pool.len(), 1); // Still alive because of second handle

    // Drop second handle
    drop(managed_value2);
    assert_eq!(pool.len(), 0); // Now removed
}

#[test]
fn ptr_access() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_value = pool.insert(42_u64);
    let ptr = managed_value.ptr();

    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    assert_eq!(value, 42);
}

#[test]
fn erase_type_information() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_value = pool.insert(42_u64);
    let erased = managed_value.erase();

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

    let managed_value = pool.insert(42_u64);
    let _cloned_handle = managed_value.clone();

    // This should panic because there are multiple references
    let _erased = managed_value.erase();
}

#[test]
fn different_types_same_pool() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_u32 = pool.insert(42_u32);
    let managed_f64 = pool.insert(2.5_f64);
    let managed_string = pool.insert("test".to_string());

    assert_eq!(pool.len(), 3);

    assert_eq!(*managed_u32, 42);
    assert!(((*managed_f64) - 2.5).abs() < f64::EPSILON);
    assert_eq!(*managed_string, "test");
}

#[test]
fn thread_safety_basic() {
    let pool = BlindPool::from(RawBlindPool::new());

    let handle = thread::spawn(move || {
        let managed_item = pool.insert(42_u64);
        *managed_item
    });

    let value = handle.join().unwrap();
    assert_eq!(value, 42);
}

#[test]
fn thread_safety_shared_handles() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_value = pool.insert(Arc::new(42_u64));
    let managed_clone = managed_value.clone();

    let handle = thread::spawn(move || **managed_clone);

    let value = handle.join().unwrap();
    assert_eq!(value, 42);

    // Original handle should still work
    assert_eq!(**managed_value, 42);
}

#[test]
fn drop_with_types_that_have_drop() {
    let pool = BlindPool::from(RawBlindPool::new());

    // Test with String and Vec - types that implement Drop
    let managed_string = pool.insert("hello".to_string());
    let managed_vec = pool.insert(vec![1, 2, 3, 4, 5]);

    assert_eq!(pool.len(), 2);
    assert_eq!(*managed_string, "hello");
    assert_eq!(*managed_vec, vec![1, 2, 3, 4, 5]);

    drop(managed_string);
    drop(managed_vec);

    assert_eq!(pool.len(), 0);
}

#[test]
fn clone_pool_handles() {
    let pool1 = BlindPool::from(RawBlindPool::new());
    let pool2 = pool1.clone();

    // Both pools refer to the same underlying pool
    let managed_value1 = pool1.insert(42_u32);
    assert_eq!(pool2.len(), 1);

    let managed_value2 = pool2.insert(43_u32);
    assert_eq!(pool1.len(), 2);

    // Values are accessible from both pool handles
    assert_eq!(*managed_value1, 42);
    assert_eq!(*managed_value2, 43);
}

#[test]
fn works_with_single_byte_type() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_byte = pool.insert(42_u8);
    assert_eq!(*managed_byte, 42);
    assert_eq!(pool.len(), 1);

    drop(managed_byte);
    assert_eq!(pool.len(), 0);
}

#[test]
fn string_methods_through_deref() {
    let pool = BlindPool::from(RawBlindPool::new());

    let managed_string = pool.insert("hello world".to_string());

    // Test that we can call String methods directly
    assert_eq!(managed_string.len(), 11);
    assert!(managed_string.starts_with("hello"));
    assert!(managed_string.ends_with("world"));
    assert_eq!(managed_string.chars().count(), 11);
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
