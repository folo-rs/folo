// This test should fail to compile because we're trying to insert a non-static reference into LocalBlindPool
use blind_pool::LocalBlindPool;

fn main() {
    let pool = LocalBlindPool::new();
    let value = 42;
    let reference = &value; // This is not 'static
    
    // This should fail to compile due to the 'static requirement
    let _handle = pool.insert(reference);
}