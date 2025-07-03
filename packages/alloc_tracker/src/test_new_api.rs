use std::alloc::System;

use alloc_tracker::{Allocator, Session};

#[global_allocator]
static ALLOCATOR: Allocator<s> = Allocator::system();

fn main() {
    let mut session = Session::new();

    let mut string_op = session.operation("do_stuff_with_strings");

    for _ in 0..3 {
        let _span = string_op.span();
        // TODO: Some string stuff here that we want to analyze.    
    }

    // Output statistics of all operations to console.
    println!("{session}");
}
