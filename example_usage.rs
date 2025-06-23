use std::alloc::Layout;
use memory_slab::{MemorySlab, MemorySlabInsertion};

fn main() {
    let layout = Layout::new::<u64>();
    let mut slab = MemorySlab::<5>::new(layout);

    // Old API would have been: let (index, ptr) = slab.insert();
    // New API with named fields:
    let insertion = slab.insert();
    
    println!("Allocated at index: {}", insertion.index);
    
    unsafe {
        // Write to the memory
        insertion.ptr.cast::<u64>().as_ptr().write(0xDEADBEEF);
        
        // Read it back via the pointer
        let value_via_ptr = insertion.ptr.cast::<u64>().as_ptr().read();
        println!("Value via pointer: {:#x}", value_via_ptr);
        
        // Read it back via index lookup
        let value_via_index = slab.get(insertion.index).cast::<u64>().as_ptr().read();
        println!("Value via index: {:#x}", value_via_index);
    }
    
    // Later operations use the index
    slab.remove(insertion.index);
    
    println!("Removed item at index {}", insertion.index);
    println!("Slab now has {} items", slab.len());
}
