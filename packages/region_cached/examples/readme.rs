//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This shows how to use `region_cached!` for memory region caching.

// `RegionCachedExt` provides required extension methods on region-cached
// static variables, such as `with_cached()` and `set_global()`.
use region_cached::{RegionCachedExt, region_cached};

region_cached!(static FAVORITE_COLOR: String = "blue".to_string());

fn main() {
    println!("=== Region Cached README Example ===");

    FAVORITE_COLOR.with_cached(|color| {
        println!("My favorite color is {color}");
    });

    FAVORITE_COLOR.set_global("red".to_string());

    FAVORITE_COLOR.with_cached(|color| {
        println!("My favorite color is now {color}");
    });

    println!("README example completed successfully!");
}
