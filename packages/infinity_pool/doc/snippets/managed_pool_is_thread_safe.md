# Thread safety

The pool is thread-safe (`Send` and `Sync`) and requires that any inserted items are `Send`.