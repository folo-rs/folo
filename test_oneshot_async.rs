// Test file to explore oneshot async API
use oneshot;

fn main() {
    let (sender, receiver) = oneshot::channel::<i32>();
    
    // Let's see what methods are available on receiver
    // Ctrl+Space in IDE would show: recv(), try_recv(), and potentially async methods
    
    // Try some potential async methods
    // receiver.async_recv(); // Does this exist?
    // receiver.recv_async(); // Does this exist?
    
    println!("Testing oneshot API");
}
