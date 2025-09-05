Removes an object from the pool and returns it.

# Panics

Panics if the handle does not reference an object in this pool.

Panics if the handle has been type-erased (`T` is `()`).