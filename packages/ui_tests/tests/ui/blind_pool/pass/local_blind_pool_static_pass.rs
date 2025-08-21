// This test should compile successfully because we're inserting owned data into LocalBlindPool
use blind_pool::LocalBlindPool;

fn main() {
    let pool = LocalBlindPool::new();
    
    // These should all work fine since the data is owned and thus 'static
    let _handle1 = pool.insert(42);
    let _handle2 = pool.insert("hello".to_string());
    let _handle3 = pool.insert(vec![1, 2, 3]);
    
    // Static references should also work
    let _handle4 = pool.insert("static string literal");
}