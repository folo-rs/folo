Transforms the unique handle into a shared handle that can be cloned as needed.

A shared handle does not support the creation of exclusive references to the target object.

# Thread Safety

The resulting shared handle will only be `Send` if `T: Send + Sync`. This is a stronger
requirement than for unique handles, which only require `T: Send`.