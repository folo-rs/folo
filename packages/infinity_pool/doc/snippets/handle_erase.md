Erases the type of the target object.

The returned handle remains functional for most purposes, just without type information.

As an important exception, a type-erased handle cannot be used to retrieve the target
object from the pool as the handle no longer has any knowledge of the type to be returned.
