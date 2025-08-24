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

/// Demonstrates using trait objects with concrete types.
fn demonstrate_animal_trait_objects() {
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
    let dog_ref: &Dog = &pooled_dog;
    let animal_trait: &dyn Animal = dog_ref;
    introduce_animal(animal_trait);

    println!();
}

/// Demonstrates mutable trait objects for training animals.
fn demonstrate_mutable_trait_objects() {
    println!("Example 2: Mutable trait objects");
    println!("--------------------------------");

    let mut dog_pool = OpaquePool::builder().layout_of::<Dog>().build();

    let dog = Dog {
        name: "Rex".to_string(),
        breed: "German Shepherd".to_string(),
    };

    // SAFETY: Dog matches the layout used to create the dog_pool.
    let mut pooled_dog = unsafe { dog_pool.insert(dog) };

    // Train the dog using mutable trait objects.
    let dog_ref: &mut Dog = &mut pooled_dog;
    let trainable: &mut dyn Trainable = dog_ref;

    println!("Before training: {}", trainable.show_training());
    trainable.train("sit");
    println!("After training: {}", trainable.show_training());

    println!();
}

fn main() {
    println!("OpaquePool Trait Object Example");
    println!("===============================");
    println!();

    demonstrate_animal_trait_objects();
    demonstrate_mutable_trait_objects();

    println!("Example completed successfully!");
    println!();
    println!("Key insights:");
    println!("- OpaquePool can work with trait objects by creating references from raw pointers");
    println!("- The caller must track the concrete type of each pooled item");
    println!("- Trait object conversion happens at access time, not storage time");
}
