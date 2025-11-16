# Implications of reference counted handles

The handle can be used to access the pooled object.

This is a reference-counted handle that automatically removes the object when the
handle is dropped. Dropping the handle is the only way to remove the object from
the pool.