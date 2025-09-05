Returns an iterator over all objects in the pool.

The iterator yields untyped pointers (`NonNull<()>`) to the objects stored in the pool.
It is the caller's responsibility to cast these pointers to the appropriate type.