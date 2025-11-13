# Thread safety

The handle provides access to an object of type `T`, so its thread-safety characteristics
are determined by the type of the object it references.

If the underlying object `T` is `Send` then the handle is `Send`.
If the underlying object `T` is `Sync` then the handle is `Sync`.