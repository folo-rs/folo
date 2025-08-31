/// A pool of objects of uniform memory layout.
/// 
/// This is the building block for creating different kinds of object pools. Underneath, they all
/// place their items in this pool, which manages the memory capacity and places items into that
/// capacity.
/// 
/// The pool does not care what the type of the items is, only that the types all match the memory
/// layout specified at creation time. This is a safety requirement of the pool APIs, guaranteed
/// by the higher-level pool types that internally use this pool.
#[derive(Debug)]
pub(crate) struct Pool {

}