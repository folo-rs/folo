//! Example demonstrating the usage of `BlindPool` for storing different types with copyable handles.

use std::f32::consts::PI;
use std::f64::consts::E;

use blind_pool::BlindPool;

fn main() {
    println!("BlindPool Example");
    println!("================");

    // Create a new blind pool that can store any type.
    let mut pool = BlindPool::new();

    println!();
    println!("Created empty BlindPool:");
    println!("  Length: {}", pool.len());
    println!("  u32 capacity: {}", pool.capacity_of::<u32>());

    // Insert different types into the same pool.
    println!();
    println!("Inserting different types...");
    let handle_u32 = pool.insert(42_u32);
    let handle_u64 = pool.insert(1234567890_u64);
    let handle_f32 = pool.insert(PI);
    let handle_f64 = pool.insert(E);
    let handle_bool = pool.insert(true);
    let handle_char = pool.insert('X');

    println!("  Inserted u32: 42");
    println!("  Inserted u64: 1234567890");
    println!("  Inserted f32: PI");
    println!("  Inserted f64: E");
    println!("  Inserted bool: true");
    println!("  Inserted char: 'X'");

    println!();
    println!("Pool status after insertions:");
    println!("  Length: {}", pool.len());
    println!("  u32 capacity: {}", pool.capacity_of::<u32>());
    println!("  f64 capacity: {}", pool.capacity_of::<f64>());

    // Demonstrate that handles act like super-powered pointers - they can be copied freely.
    println!();
    println!("Demonstrating copyable handles...");
    let handle_u32_copy = handle_u32;
    #[expect(
        clippy::clone_on_copy,
        reason = "demonstrating that Clone is available"
    )]
    let handle_f64_clone = handle_f64.clone();

    println!("  Created copies of u32 and f64 handles");

    // Read values back from the pool using the handles.
    println!();
    println!("Reading values back from the pool:");

    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_u32_original = unsafe { handle_u32.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_u32_copy = unsafe { handle_u32_copy.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_u64 = unsafe { handle_u64.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_f32 = unsafe { handle_f32.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_f64_original = unsafe { handle_f64.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_f64_clone = unsafe { handle_f64_clone.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_bool = unsafe { handle_bool.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_char = unsafe { handle_char.ptr().read() };

    println!("  u32 value (original): {val_u32_original}");
    println!("  u32 value (copy): {val_u32_copy}");
    println!("  u64 value: {val_u64}");
    println!("  f32 value: {val_f32}");
    println!("  f64 value (original): {val_f64_original}");
    println!("  f64 value (clone): {val_f64_clone}");
    println!("  bool value: {val_bool}");
    println!("  char value: '{val_char}'");

    // All copies should refer to the same values.
    assert_eq!(val_u32_original, val_u32_copy);
    assert!((val_f64_original - val_f64_clone).abs() < f64::EPSILON);

    // Demonstrate type erasure.
    println!();
    println!("Demonstrating type erasure...");
    let erased_u32 = handle_u32.erase();
    let erased_f64 = handle_f64.erase();

    // Can still access raw pointers after type erasure.
    // SAFETY: We know the types of these erased handles.
    let val = unsafe { erased_u32.ptr().cast::<u32>().read() };
    println!("  Erased u32 value: {val}");

    // SAFETY: We know the types of these erased handles.
    let val = unsafe { erased_f64.ptr().cast::<f64>().read() };
    println!("  Erased f64 value: {val}");

    // Remove some items from the pool.
    println!();
    println!("Removing some items...");
    pool.remove(erased_u32); // Remove the erased u32
    pool.remove(handle_u64);
    pool.remove(erased_f64); // Remove the erased f64

    println!("  Removed u32, u64, and f64");
    println!("  Pool length now: {}", pool.len());

    // Insert more items of existing types.
    println!();
    println!("Inserting more items of existing types...");
    let handle_f32_2 = pool.insert(2.5_f32);
    let handle_bool_2 = pool.insert(false);

    println!("  Inserted another f32: 2.5");
    println!("  Inserted another bool: false");
    println!("  Pool length now: {}", pool.len());

    // Clean up remaining items.
    println!();
    println!("Cleaning up remaining items...");
    pool.remove(handle_f32);
    pool.remove(handle_bool);
    pool.remove(handle_char);
    pool.remove(handle_f32_2);
    pool.remove(handle_bool_2);

    println!("  Pool is now empty: {}", pool.is_empty());

    // Demonstrate shrink_to_fit.
    println!();
    println!("Calling shrink_to_fit to release unused capacity...");
    let u32_capacity_before = pool.capacity_of::<u32>();
    pool.shrink_to_fit();
    let u32_capacity_after = pool.capacity_of::<u32>();
    println!("  u32 capacity before shrink: {u32_capacity_before}");
    println!("  u32 capacity after shrink: {u32_capacity_after}");
    println!();
    println!("Example completed successfully!");
}
