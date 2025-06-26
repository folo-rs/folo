//! Trait object usage with `PinnedPool`.
//!
//! This example demonstrates how to use trait objects with `PinnedPool`, showing:
//! * Storing different types that implement the same trait
//! * Converting pinned references to trait object references
//! * Using trait methods on pooled items

use std::fmt::Display;

use pinned_pool::PinnedPool;

// Define a trait that we'll use for trait objects.
trait Drawable {
    fn draw(&self) -> String;
    fn area(&self) -> f64;
}

// Different types implementing the trait.
#[derive(Debug)]
struct Circle {
    radius: f64,
}

impl Drawable for Circle {
    fn draw(&self) -> String {
        format!("Drawing circle with radius {}", self.radius)
    }

    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
}

impl Display for Circle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Circle(radius={})", self.radius)
    }
}

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

impl Display for Rectangle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Rectangle({}x{})", self.width, self.height)
    }
}

fn main() {
    println!("PinnedPool Trait Object Example");
    println!("===============================");
    println!();

    // Method 1: Store trait objects directly in the pool.
    println!("Method 1: Storing trait objects directly");
    println!("----------------------------------------");

    let mut shape_pool = PinnedPool::<Box<dyn Drawable>>::new();

    // Insert different shapes as trait objects.
    let circle_key = shape_pool.insert(Box::new(Circle { radius: 5.0 }) as Box<dyn Drawable>);
    let rectangle_key = shape_pool.insert(Box::new(Rectangle {
        width: 10.0,
        height: 6.0,
    }) as Box<dyn Drawable>);

    println!("Inserted {} shapes into pool", shape_pool.len());
    println!();

    // Access shapes via trait objects.
    let circle_shape = shape_pool.get(circle_key);
    let rectangle_shape = shape_pool.get(rectangle_key);

    println!("Circle: {}", circle_shape.draw());
    println!("Circle area: {:.2}", circle_shape.area());
    println!();

    println!("Rectangle: {}", rectangle_shape.draw());
    println!("Rectangle area: {:.2}", rectangle_shape.area());
    println!();

    // Clean up Method 1.
    shape_pool.remove(circle_key);
    shape_pool.remove(rectangle_key);

    // Method 2: Store concrete types and convert to trait objects when accessing.
    println!("Method 2: Converting to trait objects on access");
    println!("-----------------------------------------------");

    let mut circle_pool = PinnedPool::<Circle>::new();
    let mut rectangle_pool = PinnedPool::<Rectangle>::new();

    // Insert concrete types.
    let circle_key = circle_pool.insert(Circle { radius: 3.5 });
    let rectangle_key = rectangle_pool.insert(Rectangle {
        width: 8.0,
        height: 4.0,
    });

    println!("Inserted shapes into separate typed pools");
    println!();

    // Function that works with any Drawable trait object.
    fn process_drawable(drawable: &dyn Drawable) {
        println!("Processing: {}", drawable.draw());
        println!("Area: {:.2}", drawable.area());
    }

    // Convert pinned references to trait objects and use them.
    {
        let circle_ref = circle_pool.get(circle_key);
        let drawable: &dyn Drawable = circle_ref.get_ref();
        process_drawable(drawable);
    }

    println!();

    {
        let rectangle_ref = rectangle_pool.get(rectangle_key);
        let drawable: &dyn Drawable = rectangle_ref.get_ref();
        process_drawable(drawable);
    }

    println!();

    // Method 3: Mutable trait objects.
    println!("Method 3: Mutable trait objects");
    println!("-------------------------------");

    // Define a trait with mutable methods.
    trait Scalable {
        fn scale(&mut self, factor: f64);
        fn get_scale_info(&self) -> String;
    }

    impl Scalable for Circle {
        fn scale(&mut self, factor: f64) {
            self.radius *= factor;
        }

        fn get_scale_info(&self) -> String {
            format!("Circle with radius {}", self.radius)
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

    // Modify shapes via mutable trait objects.
    {
        let mut circle_ref = circle_pool.get_mut(circle_key);
        let scalable: &mut dyn Scalable = circle_ref.get_mut();
        println!("Before scaling: {}", scalable.get_scale_info());
        scalable.scale(2.0);
        println!("After scaling by 2.0: {}", scalable.get_scale_info());
    }

    println!();

    {
        let mut rectangle_ref = rectangle_pool.get_mut(rectangle_key);
        let scalable: &mut dyn Scalable = rectangle_ref.get_mut();
        println!("Before scaling: {}", scalable.get_scale_info());
        scalable.scale(1.5);
        println!("After scaling by 1.5: {}", scalable.get_scale_info());
    }

    println!();

    // Verify changes persisted.
    println!("Verifying changes persisted:");
    {
        let circle_ref = circle_pool.get(circle_key);
        let drawable: &dyn Drawable = circle_ref.get_ref();
        println!("Final circle area: {:.2}", drawable.area());
    }

    {
        let rectangle_ref = rectangle_pool.get(rectangle_key);
        let drawable: &dyn Drawable = rectangle_ref.get_ref();
        println!("Final rectangle area: {:.2}", drawable.area());
    }

    // Clean up.
    circle_pool.remove(circle_key);
    rectangle_pool.remove(rectangle_key);

    println!();
    println!("Example completed successfully!");
}