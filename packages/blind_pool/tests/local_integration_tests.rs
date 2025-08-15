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
    let ptr = value_handle.ptr();

    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    assert_eq!(value, 42);
}

#[test]
fn erase_type_information() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let u64_handle = pool.insert(42_u64);
    let erased = u64_handle.erase();

    // Should still be able to access via pointer
    // SAFETY: We know this contains a u64.
    let value = unsafe { erased.ptr().cast::<u64>().read() };
    assert_eq!(value, 42);

    // Pool should still contain the item
    assert_eq!(pool.len(), 1);

    drop(erased);
    assert_eq!(pool.len(), 0);
}

#[test]
#[should_panic(expected = "cannot erase LocalPooled with multiple references")]
fn erase_with_multiple_references_panics() {
    let pool = LocalBlindPool::from(RawBlindPool::new());

    let value_handle = pool.insert(42_u64);
    let _cloned_handle = value_handle.clone();

    // This should panic because there are multiple references
    let _erased = value_handle.erase();
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

// Test that LocalPooled is NOT Send
#[test]
fn local_pooled_is_not_send() {
    // This test documents that LocalPooled<T> is not Send.
    // The types use Rc internally which is not Send.
    let pool = LocalBlindPool::from(RawBlindPool::new());
    let _handle = pool.insert(42_u32);

    // The following would not compile:
    // std::thread::spawn(move || {
    //     println!("{}", *_handle);
    // });
}

// Test that LocalPooled is NOT Sync
#[test]
fn local_pooled_is_not_sync() {
    // This test documents that LocalPooled<T> is not Sync.
    // The types use RefCell internally which is not Sync.
    let pool = LocalBlindPool::from(RawBlindPool::new());
    let _handle = pool.insert(42_u32);

    // The following would not compile if we tried to share across threads.
}

// Test that LocalBlindPool is NOT Send
#[test]
fn local_pool_is_not_send() {
    // This test documents that LocalBlindPool is not Send.
    // The types use Rc internally which is not Send.
    let _pool = LocalBlindPool::from(RawBlindPool::new());

    // The following would not compile:
    // std::thread::spawn(move || {
    //     _pool.insert(42_u32);
    // });
}

// Test that LocalBlindPool is NOT Sync
#[test]
fn local_pool_is_not_sync() {
    // This test documents that LocalBlindPool is not Sync.
    // The types use RefCell internally which is not Sync.
    let _pool = LocalBlindPool::from(RawBlindPool::new());

    // The following would not compile if we tried to share across threads.
}
