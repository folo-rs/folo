The total capacity of the pool for objects of type `T`.

This is the maximum number of objects (including current contents) that the pool can contain
without capacity extension. The pool will automatically extend its capacity if more than
this many objects of type `T` are inserted.

Capacity may be shared between different types of objects.