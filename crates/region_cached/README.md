# region_cached

On many-processor systems with multiple memory regions, there can be an extra cost associated with
accessing data in physical memory modules that are in a different memory region than the current
processor.

This crate provides the capability to cache frequently accessed shared data sets in the local memory
region, speeding up reads when the data is not already in the local processor caches. You can think
of it as an extra level of caching between L3 processor caches and main memory.

# Applicability

A positive performance impact can be seen if all of the following is true:

1. The system has multiple memory regions.
2. A shared data set is accessed from different memory regions.
3. The data set is large enough to make it unlikely that it is resident in local CPU caches.

# Usage

This crate provides the `region_cached!` macro that enhances static variables with region-local
caching behavior.

```rust
region_cached!(static LAST_UPDATE: u128 = 0);
```

# Further reading

The macro internally transforms a static variable of type `T` into a [`RegionCachedKey<T>`][1]. See
the API documentation of this type for more details.

[1]: crate::RegionCachedKey