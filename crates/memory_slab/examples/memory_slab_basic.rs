//! Basic usage example for `MemorySlab`.
//!
//! This example demonstrates how to allocate, access, and deallocate memory using
//! the `MemorySlab` type with explicit layout specification.

use std::alloc::Layout;
use memory_slab::MemorySlab;

fn main() {
    let layout = Layout::new::<u64>();
    let mut slab = MemorySlab::<5>::new(layout);

    // Insert memory and get both index and pointer
    let insertion = slab.insert();
    
    println!("Allocated at index: {}", insertion.index());
    
    // SAFETY: We know the pointer is valid and aligned for u64 since we created the layout for u64.
    // The memory is allocated and we have exclusive access to it.
    unsafe {
        // Write to the memory
        insertion.ptr().cast::<u64>().as_ptr().write(0xDEADBEEF);
    }
      // SAFETY: We just wrote to this memory location, so it contains a valid u64.
    let value_via_ptr = unsafe {
        // Read it back via the pointer
        insertion.ptr().cast::<u64>().as_ptr().read()
    };
    println!("Value via pointer: {value_via_ptr:#x}");
    
    // SAFETY: We know the memory at this index contains a valid u64.
    let value_via_index = unsafe {
        // Read it back via index lookup
        slab.get(insertion.index()).cast::<u64>().as_ptr().read()
    };
    println!("Value via index: {value_via_index:#x}");
    
    // Later operations use the index
    slab.remove(insertion.index());
    
    println!("Removed item at index {}", insertion.index());
    println!("Slab now has {} items", slab.len());
}
