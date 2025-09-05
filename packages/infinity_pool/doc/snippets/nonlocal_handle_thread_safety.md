# Thread safety

The handle provides access to the underlying object, so its thread-safety characteristics
are determined by the type of the object it points to.

If the underlying object is `Sync`, the handle is thread-mobile (`Send`). Otherwise, the
handle is single-threaded (neither `Send` nor `Sync`).