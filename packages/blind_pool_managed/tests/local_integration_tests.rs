//! Tests for the local (single-threaded) variants of managed blind pool.
//!
//! These tests verify the correctness of `LocalManagedBlindPool` and `LocalManagedPooled<T>`
//! including basic functionality, automatic cleanup, and various edge cases.

use blind_pool::BlindPool;
use blind_pool_managed::LocalManagedBlindPool;

#[test]
fn simple_insert_and_access() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_u32 = pool.insert(42_u32);
    let managed_string = pool.insert("hello".to_string());
    
    assert_eq!(*managed_u32, 42);
    assert_eq!(*managed_string, "hello");
    assert_eq!(pool.len(), 2);
}

#[test]
fn automatic_cleanup_single_handle() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    {
        let _managed_u32 = pool.insert(42_u32);
        assert_eq!(pool.len(), 1);
    }
    
    // Item should be automatically removed after drop
    assert_eq!(pool.len(), 0);
    assert!(pool.is_empty());
}

#[test]
fn automatic_cleanup_multiple_handles() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_u32 = pool.insert(42_u32);
    let cloned_handle = managed_u32.clone();
    
    assert_eq!(pool.len(), 1);
    
    // Drop first handle - item should remain
    drop(managed_u32);
    assert_eq!(pool.len(), 1);
    
    // Drop second handle - item should be removed
    drop(cloned_handle);
    assert_eq!(pool.len(), 0);
}

#[test]
fn clone_handles() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_value = pool.insert(42_u64);
    let cloned_handle = managed_value.clone();
    
    // Both handles should refer to the same value
    assert_eq!(*managed_value, 42);
    assert_eq!(*cloned_handle, 42);
    
    // Modification through one handle should be visible through the other
    // (Note: we can't actually modify since we only have shared references)
    assert_eq!(*managed_value, *cloned_handle);
}

#[test]
fn clone_pool_handles() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    let pool_clone = pool.clone();
    
    let managed_u32 = pool.insert(42_u32);
    let managed_string = pool_clone.insert("test".to_string());
    
    assert_eq!(*managed_u32, 42);
    assert_eq!(*managed_string, "test");
    assert_eq!(pool.len(), 2);
    assert_eq!(pool_clone.len(), 2); // Should be the same pool
}

#[test]
fn string_methods_through_deref() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_string = pool.insert("hello world".to_string());
    
    // Test that we can call String methods directly
    assert_eq!(managed_string.len(), 11);
    assert!(managed_string.starts_with("hello"));
    assert!(managed_string.ends_with("world"));
    assert!(managed_string.contains("lo wo"));
}

#[test]
fn different_types_same_pool() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_u32 = pool.insert(42_u32);
    let managed_f64 = pool.insert(2.5_f64);
    let managed_string = pool.insert("test".to_string());
    
    assert_eq!(pool.len(), 3);
    
    assert_eq!(*managed_u32, 42);
    assert!(((*managed_f64) - 2.5).abs() < f64::EPSILON);
    assert_eq!(*managed_string, "test");
}

#[test]
fn ptr_access() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_value = pool.insert(42_u64);
    let ptr = managed_value.ptr();
    
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let value = unsafe { ptr.read() };
    assert_eq!(value, 42);
}

#[test]
fn erase_type_information() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_u64 = pool.insert(42_u64);
    let erased = managed_u64.erase();
    
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
#[should_panic(expected = "cannot erase LocalManagedPooled with multiple references")]
fn erase_with_multiple_references_panics() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_value = pool.insert(42_u64);
    let _cloned_handle = managed_value.clone();
    
    // This should panic because there are multiple references
    let _erased = managed_value.erase();
}

#[test]
fn drop_with_types_that_have_drop() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    // Vec has a non-trivial Drop implementation
    let managed_vec = pool.insert(vec![1, 2, 3, 4, 5]);
    
    assert_eq!(managed_vec.len(), 5);
    assert_eq!(pool.len(), 1);
    
    drop(managed_vec);
    assert_eq!(pool.len(), 0);
}

#[test]
fn works_with_single_byte_type() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
    let managed_u8 = pool.insert(255_u8);
    
    assert_eq!(*managed_u8, 255);
    assert_eq!(pool.len(), 1);
    
    drop(managed_u8);
    assert_eq!(pool.len(), 0);
}

#[test]
fn large_number_of_items() {
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    
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

// Test that LocalManagedPooled is NOT Send
#[test]
fn local_managed_pooled_is_not_send() {
    // This test documents that LocalManagedPooled<T> is not Send.
    // The types use Rc internally which is not Send.
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    let _managed = pool.insert(42_u32);
    
    // The following would not compile:
    // std::thread::spawn(move || {
    //     println!("{}", *_managed);
    // });
}

// Test that LocalManagedPooled is NOT Sync  
#[test]
fn local_managed_pooled_is_not_sync() {
    // This test documents that LocalManagedPooled<T> is not Sync.
    // The types use RefCell internally which is not Sync.
    let pool = LocalManagedBlindPool::from(BlindPool::new());
    let _managed = pool.insert(42_u32);
    
    // The following would not compile if we tried to share across threads.
}

// Test that LocalManagedBlindPool is NOT Send
#[test]
fn local_managed_pool_is_not_send() {
    // This test documents that LocalManagedBlindPool is not Send.
    // The types use Rc internally which is not Send.
    let _pool = LocalManagedBlindPool::from(BlindPool::new());
    
    // The following would not compile:
    // std::thread::spawn(move || {
    //     _pool.insert(42_u32);
    // });
}

// Test that LocalManagedBlindPool is NOT Sync
#[test]
fn local_managed_pool_is_not_sync() {
    // This test documents that LocalManagedBlindPool is not Sync.
    // The types use RefCell internally which is not Sync.
    let _pool = LocalManagedBlindPool::from(BlindPool::new());
    
    // The following would not compile if we tried to share across threads.
}
