/// A non-unique non-typed handle to an object in a [`Pool`].
/// 
/// This handle can be used to access the pooled object and to remove it from the pool. It does not
/// have any lifetime management semantics - this is just a fat pointer. It does not even know the
/// type of the object.
/// 
/// Public API types will hide this behind smarter handle mechanisms that may know the type of
/// the object and may have lifetime management semantics.
pub(crate) struct PoolHandle {
}