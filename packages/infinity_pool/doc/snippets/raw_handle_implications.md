# Implications of raw handles

The handle can be used to access the pooled object, as well as to remove
it from the pool when no longer needed.

This is a raw handle that requires manual lifetime management of the pooled objects.
* Accessing the target object is only possible via unsafe code as the handle does not
  know when the pool has been dropped - the caller must guarantee the pool still exists.
* You must explicitly remove the target object from the pool when it is no longer needed.
  If the handle is merely dropped, the object it references remains in the pool until
  the pool itself is dropped.