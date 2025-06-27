//! Trait object usage with `PinnedPool`.
//!
//! This example demonstrates how to use trait objects with `PinnedPool`, showing:
//! * Storing concrete types that implement a trait
//! * Converting pinned references to trait object references
//! * Using trait methods on pooled items

use pinned_pool::PinnedPool;

// Define a trait that we'll use for trait objects.
trait Drawable {
    fn draw(&self) -> String;
    fn area(&self) -> f64;
}

// Define a trait with mutable methods.
trait Scalable {
    fn scale(&mut self, factor: f64);
    fn get_scale_info(&self) -> String;
}

// A concrete type implementing the trait.
#[derive(Debug)]
struct Rectangle {
    width: f64,
    height: f64,
}

impl Drawable for Rectangle {
    fn draw(&self) -> String {
        format!("Drawing rectangle {}x{}", self.width, self.height)
    }

    fn area(&self) -> f64 {
        self.width * self.height
    }
}

impl Scalable for Rectangle {
    fn scale(&mut self, factor: f64) {
        self.width *= factor;
        self.height *= factor;
    }

    fn get_scale_info(&self) -> String {
        format!("Rectangle {}x{}", self.width, self.height)
    }
}

// Function that works with any Drawable trait object.
fn process_drawable(drawable: &dyn Drawable) {
    println!("Processing: {}", drawable.draw());
    println!("Area: {:.2}", drawable.area());
}

fn main() {
    println!("PinnedPool Trait Object Example");
    println!("===============================");
    println!();

    // Method 1: Store concrete types and convert to trait objects when accessing.
    println!("Method 1: Converting to trait objects on access");
    println!("-----------------------------------------------");

    let mut rectangle_pool = PinnedPool::<Rectangle>::new();

    // Insert concrete types.
    let rectangle_key = rectangle_pool.insert(Rectangle {
        width: 10.0,
        height: 6.0,
    });

    println!("Inserted rectangle into pool");
    println!();

    // Convert pinned references to trait objects and use them.
    {
        let rectangle_ref = rectangle_pool.get(rectangle_key);
        let drawable: &dyn Drawable = rectangle_ref.get_ref();
        process_drawable(drawable);
    }

    println!();

    // Method 2: Mutable trait objects.
    println!("Method 2: Mutable trait objects");
    println!("-------------------------------");

    // Modify shapes via mutable trait objects.
    {
        let rectangle_ref = rectangle_pool.get_mut(rectangle_key);
        let scalable: &mut dyn Scalable = rectangle_ref.get_mut();
        println!("Before scaling: {}", scalable.get_scale_info());
        scalable.scale(1.5);
        println!("After scaling by 1.5: {}", scalable.get_scale_info());
    }

    println!();

    // Verify changes persisted.
    println!("Verifying changes persisted:");
    {
        let rectangle_ref = rectangle_pool.get(rectangle_key);
        let drawable: &dyn Drawable = rectangle_ref.get_ref();
        println!("Final rectangle area: {:.2}", drawable.area());
    }

    // Clean up.
    rectangle_pool.remove(rectangle_key);

    println!();
    println!("Example completed successfully!");
}
