//! Integration test for dynamic cast macro functionality.

use std::fmt::Display;

use blind_pool::{BlindPool, define_pooled_dyn_cast};

// Define cast operation
define_pooled_dyn_cast!(Display);

#[test]
fn dynamic_cast_integration_test() {
    let pool = BlindPool::new();
    let pooled = pool.insert(42_u32);
    let display_pooled = pooled.cast_display();

    assert_eq!(format!("{}", &*display_pooled), "42");
}
