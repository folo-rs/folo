// This test should fail to compile because we're trying to use insert_mut with non-static data in BlindPool
use blind_pool::BlindPool;

fn main() {
    let pool = BlindPool::new();
    let value = 42;
    let reference = &value; // This is not 'static
    
    // This should fail to compile due to the 'static requirement
    let _handle = pool.insert_mut(reference);
}