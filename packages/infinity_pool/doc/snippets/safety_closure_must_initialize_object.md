The closure must correctly initialize the object. All fields that
are not `MaybeUninit` must be initialized when the closure returns.