//! Trait object usage with `OpaquePool`.
//!
//! This example demonstrates how to use trait objects with `OpaquePool`, showing:
//! * Creating references from raw pointers
//! * Converting those references to trait object references
//! * Using trait methods on pooled items

use std::alloc::Layout;
use std::fmt::Display;

use opaque_pool::OpaquePool;

// Define traits for our examples.
trait Animal {
    fn speak(&self) -> String;
    fn name(&self) -> &str;
}

trait Drawable {
    fn draw(&self) -> String;
    fn area(&self) -> f64;
}

// Animal implementations.
#[derive(Debug)]
struct Dog {
    name: String,
    breed: String,
}

impl Animal for Dog {
    fn speak(&self) -> String {
        format!("Woof! I'm {}, a {}", self.name, self.breed)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Display for Dog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.name, self.breed)
    }
}

#[derive(Debug)]
struct Cat {
    name: String,
    indoor: bool,
}

impl Animal for Cat {
    fn speak(&self) -> String {
        let location = if self.indoor { "indoors" } else { "outdoors" };
        format!("Meow! I'm {}, living {}", self.name, location)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

impl Display for Cat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let location = if self.indoor { "indoor" } else { "outdoor" };
        write!(f, "{} ({} cat)", self.name, location)
    }
}

// Shape implementations.
#[derive(Debug)]
struct Circle {
    radius: f64,
}

impl Drawable for Circle {
    fn draw(&self) -> String {
        format!("Drawing circle with radius {:.1}", self.radius)
    }

    fn area(&self) -> f64 {
        std::f64::consts::PI * self.radius * self.radius
    }
}

#[derive(Debug)]
struct Square {
    side: f64,
}

impl Drawable for Square {
    fn draw(&self) -> String {
        format!("Drawing square with side {:.1}", self.side)
    }

    fn area(&self) -> f64 {
        self.side * self.side
    }
}

fn main() {
    println!("OpaquePool Trait Object Example");
    println!("===============================");
    println!();

    // Example 1: Using trait objects with different types in separate pools.
    println!("Example 1: Animal trait objects");
    println!("-------------------------------");

    // Create separate pools for different animal types.
    let mut dog_pool = OpaquePool::builder().layout_of::<Dog>().build();
    let mut cat_pool = OpaquePool::builder().layout_of::<Cat>().build();

    // Insert animals.
    let dog = Dog {
        name: "Buddy".to_string(),
        breed: "Golden Retriever".to_string(),
    };
    let cat = Cat {
        name: "Whiskers".to_string(),
        indoor: true,
    };

    // SAFETY: Dog matches the layout used to create the dog_pool.
    let pooled_dog = unsafe { dog_pool.insert(dog) };
    // SAFETY: Cat matches the layout used to create the cat_pool.
    let pooled_cat = unsafe { cat_pool.insert(cat) };

    println!("Inserted dog and cat into their respective pools");

    // Function that works with any Animal trait object.
    fn introduce_animal(animal: &dyn Animal) {
        println!("Animal introduction: {}", animal.speak());
        println!("Animal name: {}", animal.name());
    }

    // Use the animals as trait objects.
    unsafe {
        // SAFETY: The pointer is valid and points to a Dog that we just inserted.
        let dog_ref: &Dog = pooled_dog.ptr().as_ref();
        let animal_trait: &dyn Animal = dog_ref;
        introduce_animal(animal_trait);
    }

    println!();

    unsafe {
        // SAFETY: The pointer is valid and points to a Cat that we just inserted.
        let cat_ref: &Cat = pooled_cat.ptr().as_ref();
        let animal_trait: &dyn Animal = cat_ref;
        introduce_animal(animal_trait);
    }

    println!();

    // Example 2: Mutable trait objects.
    println!("Example 2: Mutable trait objects");
    println!("--------------------------------");

    trait Trainable {
        fn train(&mut self, command: &str);
        fn show_training(&self) -> String;
    }

    #[derive(Debug)]
    struct TrainableDog {
        name: String,
        commands: Vec<String>,
    }

    impl Trainable for TrainableDog {
        fn train(&mut self, command: &str) {
            self.commands.push(command.to_string());
        }

        fn show_training(&self) -> String {
            if self.commands.is_empty() {
                format!("{} knows no commands yet", self.name)
            } else {
                format!(
                    "{} knows these commands: {}",
                    self.name,
                    self.commands.join(", ")
                )
            }
        }
    }

    let mut trainable_pool = OpaquePool::builder().layout_of::<TrainableDog>().build();

    let trainable_dog = TrainableDog {
        name: "Max".to_string(),
        commands: Vec::new(),
    };

    // SAFETY: TrainableDog matches the layout used to create the trainable_pool.
    let pooled_trainable = unsafe { trainable_pool.insert(trainable_dog) };

    // Train the dog using mutable trait objects.
    unsafe {
        // SAFETY: The pointer is valid and points to a TrainableDog that we just inserted.
        let dog_ref: &mut TrainableDog = pooled_trainable.ptr().as_mut();
        let trainable: &mut dyn Trainable = dog_ref;

        println!("Before training: {}", trainable.show_training());
        trainable.train("sit");
        trainable.train("stay");
        trainable.train("fetch");
        println!("After training: {}", trainable.show_training());
    }

    println!();

    // Example 3: Mixed type pools with trait objects.
    println!("Example 3: Shapes with same layout");
    println!("----------------------------------");

    // Both Circle and Square have the same memory layout (one f64),
    // so they can use the same pool if we're careful about types.
    let circle_layout = Layout::new::<Circle>();
    let square_layout = Layout::new::<Square>();

    println!("Circle layout: {:?}", circle_layout);
    println!("Square layout: {:?}", square_layout);

    if circle_layout == square_layout {
        println!("Layouts match! Can use the same pool.");

        let mut shape_pool = OpaquePool::builder().layout(circle_layout).build();

        let circle = Circle { radius: 5.0 };
        let square = Square { side: 4.0 };

        // SAFETY: Both Circle and Square have the same layout as the pool.
        let pooled_circle = unsafe { shape_pool.insert(circle) };
        // SAFETY: Both Circle and Square have the same layout as the pool.
        let pooled_square = unsafe { shape_pool.insert(square) };

        // We need to keep track of which type each handle represents.
        // In a real application, you might use an enum or separate collections.

        println!("Inserted circle and square into the same pool");

        // Use shapes as trait objects.
        fn describe_shape(shape: &dyn Drawable) {
            println!("Shape: {}", shape.draw());
            println!("Area: {:.2}", shape.area());
        }

        // Access circle as trait object.
        unsafe {
            // SAFETY: We know this handle points to a Circle.
            let circle_ref: &Circle = pooled_circle.ptr().as_ref();
            let drawable: &dyn Drawable = circle_ref;
            describe_shape(drawable);
        }

        println!();

        // Access square as trait object.
        unsafe {
            // SAFETY: We know this handle points to a Square.
            let square_ref: &Square = pooled_square.ptr().as_ref();
            let drawable: &dyn Drawable = square_ref;
            describe_shape(drawable);
        }

        // Clean up shapes.
        shape_pool.remove(pooled_circle);
        shape_pool.remove(pooled_square);
    } else {
        println!("Layouts don't match - would need separate pools.");
    }

    println!();

    // Clean up animals.
    dog_pool.remove(pooled_dog);
    cat_pool.remove(pooled_cat);
    trainable_pool.remove(pooled_trainable);

    println!("Example completed successfully!");
    println!();
    println!("Key insights:");
    println!("- OpaquePool can work with trait objects by creating references from raw pointers");
    println!("- The caller must track the concrete type of each pooled item");
    println!("- Types with the same layout can share the same pool");
    println!("- Trait object conversion happens at access time, not storage time");
}