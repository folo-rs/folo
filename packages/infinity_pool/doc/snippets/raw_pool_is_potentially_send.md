The pool is single-threaded, though if all the objects inserted are `Send` then the owner of
the pool is allowed to treat the pool itself as `Send` (but must do so via a wrapper type that
implements `Send` using unsafe code).