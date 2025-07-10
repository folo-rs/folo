//! Verification example showing the difference between writing full structs vs field-by-field.

use std::mem::MaybeUninit;

#[allow(
    dead_code,
    reason = "memory field is used for demonstration but not accessed"
)]
struct HalfFull {
    value: usize,
    memory: [MaybeUninit<u8>; 16],
}

fn main() {
    println!("This example verifies that we can initialize only part of a struct");

    // Method 1: Writing the full struct (this DOES write to memory field)
    let mut uninit1: MaybeUninit<HalfFull> = MaybeUninit::uninit();
    uninit1.write(HalfFull {
        value: 42,
        memory: [MaybeUninit::uninit(); 16], // This actually writes bytes!
    });

    // SAFETY: We properly initialized the struct above.
    let initialized1 = unsafe { uninit1.assume_init() };
    println!(
        "Method 1 (full struct write): value = {}",
        initialized1.value
    );

    // Method 2: Writing only specific fields (this does NOT write to memory field)
    let mut uninit2: MaybeUninit<HalfFull> = MaybeUninit::uninit();
    let ptr = uninit2.as_mut_ptr();
    // SAFETY: ptr points to valid memory and we're dereferencing to take address of a field.
    let value_ptr = unsafe { &raw mut (*ptr).value };
    // SAFETY: value_ptr points to valid memory for a usize.
    unsafe {
        value_ptr.write(42);
    }
    // Note: We deliberately do NOT write to the memory field

    // SAFETY: We initialized the required field above, leaving memory field uninitialized
    // which is safe because MaybeUninit<T> doesn't require initialization.
    let initialized2 = unsafe { uninit2.assume_init() };
    println!("Method 2 (field-by-field): value = {}", initialized2.value);

    println!("Both methods work, but Method 2 truly leaves memory field uninitialized");
}
