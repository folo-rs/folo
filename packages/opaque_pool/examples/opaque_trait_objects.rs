//! Trait object usage with `OpaquePool`.
//!
//! This example demonstrates how to use trait objects with `OpaquePool`, showing:
//! * Creating references from raw pointers
//! * Converting those references to trait object references
//! * Using trait methods on pooled items

use opaque_pool::OpaquePool;

// Define a trait for our example.
trait Animal {
    fn speak(&self) -> String;
    fn name(&self) -> &str;
}

// Define another trait for training.
trait Trainable {
    fn train(&mut self, command: &str);
    fn show_training(&self) -> String;
}

// Animal implementation.
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

impl Trainable for Dog {
    fn train(&mut self, command: &str) {
        // In this simple example, we'll just modify the breed to show training
        self.breed = format!("{} (trained: {})", self.breed, command);
    }

    fn show_training(&self) -> String {
        format!("{} training status: {}", self.name, self.breed)
    }
}

// Function that works with any Animal trait object.
fn introduce_animal(animal: &dyn Animal) {
    println!("Animal introduction: {}", animal.speak());
    println!("Animal name: {}", animal.name());
}

fn main() {
    println!("OpaquePool Trait Object Example");
    println!("===============================");
    println!();

    // Example 1: Using trait objects with concrete types.
    println!("Example 1: Animal trait objects");
    println!("-------------------------------");

    // Create a pool for Dog types.
    let mut dog_pool = OpaquePool::builder().layout_of::<Dog>().build();

    // Insert a dog.
    let dog = Dog {
        name: "Buddy".to_string(),
        breed: "Golden Retriever".to_string(),
    };

    // SAFETY: Dog matches the layout used to create the dog_pool.
    let pooled_dog = unsafe { dog_pool.insert(dog) };

    println!("Inserted dog into pool");

    // Use the dog as a trait object.
    // SAFETY: The pointer is valid and points to a Dog that we just inserted.
    unsafe {
        let dog_ref: &Dog = pooled_dog.ptr().as_ref();
        let animal_trait: &dyn Animal = dog_ref;
        introduce_animal(animal_trait);
    }

    println!();

    // Example 2: Mutable trait objects.
    println!("Example 2: Mutable trait objects");
    println!("--------------------------------");

    // Train the dog using mutable trait objects.
    // SAFETY: The pointer is valid and points to a Dog that we just inserted.
    unsafe {
        let dog_ref: &mut Dog = pooled_dog.ptr().as_mut();
        let trainable: &mut dyn Trainable = dog_ref;

        println!("Before training: {}", trainable.show_training());
        trainable.train("sit");
        println!("After training: {}", trainable.show_training());
    }

    println!();

    // Clean up.
    dog_pool.remove(pooled_dog);

    println!("Example completed successfully!");
    println!();
    println!("Key insights:");
    println!("- OpaquePool can work with trait objects by creating references from raw pointers");
    println!("- The caller must track the concrete type of each pooled item");
    println!("- Trait object conversion happens at access time, not storage time");
}
