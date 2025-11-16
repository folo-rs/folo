Borrows the target object via shared reference.

# Safety

The caller must guarantee that the pool remains alive for
the duration the returned reference is used.

The caller must guarantee that the object remains in the pool
for the duration the returned reference is used.