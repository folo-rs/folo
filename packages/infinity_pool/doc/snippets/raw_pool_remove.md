Removes an object from the pool, dropping it.

# Panics

Panics if the handle does not reference an object in this pool.

# Safety

The caller must ensure that the handle belongs to this pool and that the object it
references has not already been removed from the pool.