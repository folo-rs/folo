// This test should fail to compile because we're trying to insert a non-static reference into BlindPool
use blind_pool::BlindPool;

fn main() {
    let pool = BlindPool::new();
    let value = 42;
    let reference = &value; // This is not 'static
    
    // This should fail to compile due to the 'static requirement
    let _handle = pool.insert(reference);
}