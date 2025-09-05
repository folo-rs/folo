Removes an object from the pool and returns it.

# Panics

Panics if the handle does not reference an existing object in this pool.

Panics if the handle has been type-erased (`T` is `()`).

# Safety

The caller must ensure that the handle belongs to this pool and that the object it
references has not already been removed from the pool.