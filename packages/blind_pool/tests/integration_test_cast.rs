//! Integration test for dynamic cast macro functionality.

use blind_pool::{BlindPool, define_pooled_dyn_cast};
use std::fmt::Display;

// Define cast operation
define_pooled_dyn_cast!(display, Display);

#[test]
fn dynamic_cast_integration_test() {
    let pool = BlindPool::new();
    let pooled = pool.insert(42_u32);
    let display_pooled = pooled.cast_display();

    assert_eq!(format!("{}", &*display_pooled), "42");
}
