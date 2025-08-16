//! Tests for the pooled dynamic cast functionality.

use std::fmt::Display;

use blind_pool::{BlindPool, LocalBlindPool, RawBlindPool, define_pooled_dyn_cast};

// Define the cast operation for Display
define_pooled_dyn_cast!(Display);

#[test]
fn pooled_cast_display() {
    let pool = BlindPool::new();
    let pooled = pool.insert(42_u32);

    // Cast to trait object while preserving reference counting.
    let display_pooled = pooled.cast_display();

    // Verify we can use it as Display
    assert_eq!(format!("{}", &*display_pooled), "42");
}

#[test]
fn local_pooled_cast_display() {
    let pool = LocalBlindPool::new();
    let pooled = pool.insert(42_u32);

    // Cast to trait object while preserving reference counting.
    let display_pooled = pooled.cast_display();

    // Verify we can use it as Display
    assert_eq!(format!("{}", &*display_pooled), "42");
}

#[test]
fn raw_pooled_cast_display() {
    let mut pool = RawBlindPool::new();
    let pooled = pool.insert(42_u32);

    // Cast to trait object.
    let display_pooled = pooled.cast_display();

    // Verify we can access the trait object
    // SAFETY: The pointer is valid and contains the value we just inserted.
    let display_str = unsafe { format!("{}", display_pooled.ptr().as_ref()) };
    assert_eq!(display_str, "42");

    // Clean up
    pool.remove(display_pooled);
}

#[test]
fn struct_storage() {
    #[allow(dead_code, reason = "Test struct to verify compilation")]
    struct Foo {
        display_value: blind_pool::Pooled<dyn Display>,
    }

    let pool = BlindPool::new();
    let pooled = pool.insert(42_u32);
    let display_pooled = pooled.cast_display();

    let foo = Foo {
        display_value: display_pooled,
    };
    // Foo will automatically clean up when dropped
    drop(foo);
}
