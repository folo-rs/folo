//! Example that demonstrates the exact usage shown in the README.md file.
//!
//! This example creates non-zero integers using the `nz!` macro as shown in the documentation.

use std::num::NonZero;
use new_zealand::nz;

fn foo(x: NonZero<u32>) { 
    println!("NonZero value: {x}"); 
}

fn main() {
    println!("=== New Zealand README Example ===");
    
    // Call function with non-zero value created using the nz! macro
    foo(nz!(42));
    
    // Demonstrate that it works with other values too
    foo(nz!(1));
    foo(nz!(100));
    
    // Demonstrate that it's a compile-time check
    let compile_time_value = nz!(123);
    println!("Compile-time non-zero value: {compile_time_value}");
    
    println!("README example completed successfully!");
}
