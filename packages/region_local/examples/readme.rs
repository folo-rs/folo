//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `region_local!` for memory region isolation.

// `RegionLocalExt` provides required extension methods on region-local
// static variables, such as `with_local()` and `set_local()`.
use region_local::{RegionLocalExt, region_local};

region_local!(static FAVORITE_COLOR: String = "blue".to_string());

fn main() {
    println!("=== Region Local README Example ===");

    FAVORITE_COLOR.with_local(|color| {
        println!("My favorite color is {color}");
    });

    FAVORITE_COLOR.set_local("red".to_string());

    FAVORITE_COLOR.with_local(|color| {
        println!("My favorite color is now {color}");
    });

    println!("README example completed successfully!");
}
