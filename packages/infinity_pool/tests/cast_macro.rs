//! Tests of the `define_pooled_dyn_cast!` macro, separate from the main crate
//! just to prove that this does not depend on accessing any private APIs.

use std::fmt::Display;

use infinity_pool::{
    BlindPool, LocalBlindPool, LocalPinnedPool, PinnedPool, RawBlindPoolBuilder,
    RawOpaquePoolBuilder, RawPinnedPoolBuilder, define_pooled_dyn_cast,
};

// Define cast for Display trait to test the macro
define_pooled_dyn_cast!(Display);

// ========== Tests for all 8 handle types ==========

#[test]
fn cast_managed_blind_pooled_to_display_trait() {
    let pool = BlindPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_blind_pooled_mut_to_display_trait() {
    let pool = BlindPool::new();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_blind_pooled_to_display_trait() {
    let pool = LocalBlindPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_blind_pooled_mut_to_display_trait() {
    let pool = LocalBlindPool::new();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_pinned_pooled_to_display_trait() {
    let pool = PinnedPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_managed_pinned_pooled_mut_to_display_trait() {
    let pool = PinnedPool::new();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_pinned_pooled_to_display_trait() {
    let pool = LocalPinnedPool::new();
    let pooled = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_local_pinned_pooled_mut_to_display_trait() {
    let pool = LocalPinnedPool::new();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object while preserving reference counting
    let display_pooled = pooled_mut.cast_display();

    // Verify the cast worked
    assert_eq!(display_pooled.to_string(), "Test string");

    // Drop the cast handle and verify pool is empty (reference counting works)
    drop(display_pooled);
    assert_eq!(pool.len(), 0);
}

// ========== Tests for raw pool removal via trait objects ==========

#[test]
fn cast_raw_blind_pooled_removal_shared() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let pooled = pool.insert("Test string".to_string()).into_shared();

    // Cast to trait object.
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(display_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_blind_pooled_removal_unique() {
    let mut pool = RawBlindPoolBuilder::new().build();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object.
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled_mut.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    pool.remove_mut(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_opaque_pooled_removal_shared() {
    let mut pool = RawOpaquePoolBuilder::new().layout_of::<String>().build();
    let pooled = pool.insert("Test string".to_string()).into_shared();

    // Cast to trait object - raw pooled requires unsafe
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(display_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_opaque_pooled_removal_unique() {
    let mut pool = RawOpaquePoolBuilder::new().layout_of::<String>().build();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object - raw pooled requires unsafe
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled_mut.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    pool.remove_mut(display_pooled);
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_pinned_pooled_removal_shared() {
    let mut pool = RawPinnedPoolBuilder::new().build();
    let pooled = pool.insert("Test string".to_string()).into_shared();

    // Cast to trait object - raw pooled requires unsafe
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    // SAFETY: We know the handle is valid and we own it
    unsafe {
        pool.remove(display_pooled);
    }
    assert_eq!(pool.len(), 0);
}

#[test]
fn cast_raw_pinned_pooled_removal_unique() {
    let mut pool = RawPinnedPoolBuilder::new().build();
    let pooled_mut = pool.insert("Test string".to_string());

    // Cast to trait object - raw pooled requires unsafe
    // SAFETY: We must guarantee the pool remains alive during this call - yes, we promise.
    let display_pooled = unsafe { pooled_mut.cast_display() };

    // Verify the cast worked
    // SAFETY: We know the handle is valid
    let display_ref = unsafe { display_pooled.as_ref() };
    assert_eq!(display_ref.to_string(), "Test string");

    // Remove via trait object
    pool.remove_mut(display_pooled);
    assert_eq!(pool.len(), 0);
}
