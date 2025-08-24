//! Demonstrates pinning support in `OpaquePool`.
//!
//! This example shows how to work with pinned values in the pool,
//! which is critical for self-referential types and async code.

use std::pin::Pin;
use std::ptr::NonNull;

use opaque_pool::OpaquePool;

// A truly self-referential struct that requires pinning.
// This struct contains a pointer that references data within itself.
struct SelfReferential {
    data: String,
    // This pointer references data within the same struct instance.
    self_ptr: Option<NonNull<u8>>,
}

impl SelfReferential {
    /// Creates a new instance without establishing the self-reference yet.
    /// The self-reference must be created after the value is pinned.
    fn new(content: &str) -> Self {
        Self {
            data: content.to_string(),
            self_ptr: None,
        }
    }

    /// Establishes a self-reference after the object has been pinned.
    /// This must be called only after the object is pinned and will never move.
    unsafe fn establish_self_ref(self: Pin<&mut Self>) {
        // SAFETY: This is safe because we are only setting up a self-reference
        // to our own data and the object is guaranteed to be pinned.
        let this = unsafe { self.get_unchecked_mut() };
        this.self_ptr = NonNull::new(this.data.as_ptr().cast_mut());
    }

    /// Verifies that our self-reference is pointing to our own data.
    /// This demonstrates that the self-reference is working correctly.
    fn validate_self_reference(&self) -> bool {
        self.self_ptr
            .is_some_and(|self_ptr| self_ptr.as_ptr() == self.data.as_ptr().cast_mut())
    }

    /// Returns the length of the data we are referencing.
    fn ref_len(&self) -> usize {
        self.data.len()
    }

    /// Returns the data content.
    fn data(&self) -> &str {
        &self.data
    }

    /// Returns a description that includes validation of the self-reference.
    fn get_pinned_info(self: Pin<&Self>) -> String {
        let is_valid = self.validate_self_reference();
        format!(
            "SelfRef: '{}' (self_ptr: {}, valid: {})",
            self.data(),
            if self.self_ptr.is_some() {
                "Some"
            } else {
                "None"
            },
            is_valid
        )
    }

    /// Modifies the data while maintaining the self-reference.
    /// This demonstrates safe mutation of self-referential data.
    fn modify_data(self: Pin<&mut Self>, suffix: &str) {
        // SAFETY: We're modifying the data but not moving the struct.
        // We need to update our self-pointer after modification.
        unsafe {
            let this = self.get_unchecked_mut();
            this.data.push_str(suffix);
            // Update our self-pointer to point to the new data location.
            // This is necessary because String::push_str might reallocate.
            this.self_ptr = NonNull::new(this.data.as_ptr().cast_mut());
        }
    }
}

// Demonstrate that this type is NOT safe to move after establishing self-reference.
impl Drop for SelfReferential {
    fn drop(&mut self) {
        // In a real implementation, we might want to validate that our self-reference
        // is still valid here, but for this example we'll just clear it.
        self.self_ptr = None;
    }
}

/// Demonstrates basic pinning support with shared handles.
fn demonstrate_basic_pinning() {
    println!("Example 1: Basic pinning with shared handles");
    println!("--------------------------------------------");

    let mut pool = OpaquePool::builder().layout_of::<SelfReferential>().build();

    // Insert a self-referential value.
    // SAFETY: SelfReferential matches the layout used to create the pool.
    let mut item = unsafe { pool.insert(SelfReferential::new("Hello pinning")) };

    // Establish the self-reference while the value is in the pool (pinned).
    let pinned_mut = item.as_pin_mut();
    // SAFETY: The value is now pinned in the pool and won't move.
    unsafe {
        pinned_mut.establish_self_ref();
    }

    // Convert to shared for pinning demonstrations.
    let shared_item = item.into_shared();

    // Get a pinned reference to the shared item.
    let pinned_ref: Pin<&SelfReferential> = shared_item.as_pin();
    println!("Pinned info: {}", pinned_ref.get_pinned_info());

    // The pinned reference ensures the value won't move.
    println!("Data: '{}'", pinned_ref.data());
    println!("Reference length: {}", pinned_ref.ref_len());
    println!(
        "Self-reference valid: {}",
        pinned_ref.validate_self_reference()
    );

    // Clean up.
    // SAFETY: No other copies of shared_item will be used after this call.
    unsafe {
        pool.remove(&shared_item);
    }

    println!();
}

/// Demonstrates mutable pinning with exclusive handles.
fn demonstrate_mutable_pinning() {
    println!("Example 2: Mutable pinning with exclusive handles");
    println!("-------------------------------------------------");

    let mut pool = OpaquePool::builder().layout_of::<SelfReferential>().build();

    // SAFETY: SelfReferential matches the layout used to create the pool.
    let mut exclusive_item = unsafe { pool.insert(SelfReferential::new("Mutable pinning")) };

    // Establish the self-reference.
    let pinned_mut = exclusive_item.as_pin_mut();
    // SAFETY: The value is now pinned in the pool and won't move.
    unsafe {
        pinned_mut.establish_self_ref();
    }

    // Get mutable pinned access again.
    let mut pinned_mut: Pin<&mut SelfReferential> = exclusive_item.as_pin_mut();

    println!(
        "Before modification: {}",
        pinned_mut.as_ref().get_pinned_info()
    );

    // Modify through the pinned mutable reference.
    pinned_mut.as_mut().modify_data(" - modified!");

    println!(
        "After modification: {}",
        pinned_mut.as_ref().get_pinned_info()
    );

    pool.remove_mut(exclusive_item);
    println!();
}

/// Demonstrates pinning with regular types.
fn demonstrate_regular_type_pinning() {
    println!("Example 3: Pinning regular types");
    println!("--------------------------------");

    let mut string_pool = OpaquePool::builder().layout_of::<String>().build();

    // SAFETY: String matches the layout used to create the pool.
    let mut string_item = unsafe { string_pool.insert("Pinned string".to_string()) };

    // Get pinned access to the string.
    let pinned_string: Pin<&String> = string_item.as_pin();
    println!("Pinned string: '{}'", &**pinned_string);
    println!("Pinned string length: {}", pinned_string.len());

    // Mutable pinned access.
    let pinned_string_mut: Pin<&mut String> = string_item.as_pin_mut();
    // SAFETY: String doesn't have self-references, so this is safe.
    unsafe {
        pinned_string_mut
            .get_unchecked_mut()
            .push_str(" (modified)");
    }

    println!("After modification: '{}'", &*string_item);

    string_pool.remove_mut(string_item);
    println!();
}

/// Demonstrates multiple pinned references to shared data.
fn demonstrate_multiple_pinned_references() {
    println!("Example 4: Multiple pinned references to shared data");
    println!("---------------------------------------------------");

    let mut pool = OpaquePool::builder().layout_of::<SelfReferential>().build();

    // SAFETY: SelfReferential matches the layout used to create the pool.
    let mut item = unsafe { pool.insert(SelfReferential::new("Shared pinning")) };

    // Establish the self-reference.
    let pinned_mut = item.as_pin_mut();
    // SAFETY: The value is now pinned in the pool and won't move.
    unsafe {
        pinned_mut.establish_self_ref();
    }

    // Convert to shared for multiple references.
    let shared_item = item.into_shared();

    // Create multiple copies of a shared handle.
    let shared_copy1 = shared_item;
    let shared_copy2 = shared_item;

    // Get pinned references from different handles (all point to same data).
    let pin1: Pin<&SelfReferential> = shared_copy1.as_pin();
    let pin2: Pin<&SelfReferential> = shared_copy2.as_pin();

    println!("Pin 1 info: {}", pin1.get_pinned_info());
    println!("Pin 2 info: {}", pin2.get_pinned_info());

    // Both should show the same data since they point to the same pool item.
    assert_eq!(pin1.data(), pin2.data());
    assert_eq!(pin1.ref_len(), pin2.ref_len());

    println!("Both pinned references point to the same data ✓");

    // Clean up.
    // SAFETY: No other copies of shared_item will be used after this call.
    unsafe {
        pool.remove(&shared_item);
    }

    println!();
}

fn main() {
    println!("=== OpaquePool Pinning Examples ===");
    println!();

    demonstrate_basic_pinning();
    demonstrate_mutable_pinning();
    demonstrate_regular_type_pinning();
    demonstrate_multiple_pinned_references();

    println!("Key benefits of pinning support:");
    println!("- Stable memory addresses enable safe self-referential types");
    println!("- Pin<&T> and Pin<&mut T> provide compile-time guarantees");
    println!("- Works seamlessly with both shared and exclusive handles");
    println!("- Essential for async code and advanced data structures");
    println!();
    println!("Pinning example completed successfully!");
}
