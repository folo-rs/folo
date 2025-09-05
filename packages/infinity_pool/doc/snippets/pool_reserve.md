Ensures that the pool has capacity for at least `additional` more objects.

# Panics

Panics if the new capacity would exceed the size of virtual memory (`usize::MAX`).