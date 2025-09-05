Borrows the target object as a pinned shared reference.

All pooled objects are guaranteed to be pinned for their entire lifetime.

# Safety

The caller must guarantee that the pool will remain alive for the duration the returned
reference is used.