//! Example demonstrating the usage of `BlindPool` for storing different types.

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
    println!("  Capacity: {}", pool.capacity());

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
    println!("  Capacity: {}", pool.capacity());

    // Read values back from the pool using the handles.
    println!();
    println!("Reading values back from the pool:");

    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_u32 = unsafe { handle_u32.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_u64 = unsafe { handle_u64.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_f32 = unsafe { handle_f32.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_f64 = unsafe { handle_f64.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_bool = unsafe { handle_bool.ptr().read() };
    // SAFETY: All pointers are valid and contain the values we just inserted.
    let val_char = unsafe { handle_char.ptr().read() };

    println!("  u32 value: {val_u32}");
    println!("  u64 value: {val_u64}");
    println!("  f32 value: {val_f32}");
    println!("  f64 value: {val_f64}");
    println!("  bool value: {val_bool}");
    println!("  char value: '{val_char}'");

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
    let capacity_before = pool.capacity();
    pool.shrink_to_fit();
    let capacity_after = pool.capacity();
    println!("  Capacity before shrink: {capacity_before}");
    println!("  Capacity after shrink: {capacity_after}");
    println!();
    println!("Example completed successfully!");
}
