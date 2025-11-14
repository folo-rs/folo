# Thread safety

The pool is nominally single-threaded because the compiler cannot know what types of objects
are stored inside a given instance, so it must default to assuming they are single-threaded
objects.

If all the objects inserted are `Send` then the owner of the pool is allowed to treat
the pool itself as thread-safe (`Send` and `Sync`) but must do so using unsafe code,
such as via a wrapper type that explicitly implements `Send` and `Sync`.