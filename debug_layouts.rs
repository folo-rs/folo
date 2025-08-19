//! Test to check if trait object casting changes the layout

use std::future::Future;
use std::mem::{size_of, align_of};

async fn echo(val: u32) -> u32 {
    val
}

trait MyFuture: Future<Output = u32> {}
impl<T> MyFuture for T where T: Future<Output = u32> {}

fn main() {
    let future = echo(42);
    
    println!("Size of concrete future: {}", size_of_val(&future));
    println!("Align of concrete future: {}", align_of_val(&future));
    
    let dyn_future: &dyn MyFuture = &future;
    println!("Size of trait object: {}", size_of_val(dyn_future));
    println!("Align of trait object: {}", align_of_val(dyn_future));
    
    println!("Size of trait object ref: {}", size_of::<&dyn MyFuture>());
    println!("Align of trait object ref: {}", align_of::<&dyn MyFuture>());
}