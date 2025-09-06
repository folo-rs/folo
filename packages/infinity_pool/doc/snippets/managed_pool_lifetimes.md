# Lifetime management

When inserting an object into the pool, a handle to the object is returned.
The object is removed from the pool when the last remaining handle to the object
is dropped (`Arc`-like behavior).

Clones of the pool are functionally equivalent views over the same memory capacity.
