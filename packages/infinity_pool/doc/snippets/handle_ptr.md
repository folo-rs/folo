Get a pointer to the target object.

All pooled objects are guaranteed to be pinned for their entire lifetime, so this pointer
remains valid for as long as the object remains in the pool.

The object pool implementation does not keep any references to the pooled objects, so
you have the option of using this pointer to create Rust references directly without fear
of any conflicting references created by the pool.