# Lifetime management

The pool itself acts as a handle - any clones of it are functionally equivalent,
similar to `Rc`.

When inserting an object into the pool, a handle to the object is returned.
The object is removed from the pool when the last remaining handle to the object
is dropped (`Rc`-like behavior).