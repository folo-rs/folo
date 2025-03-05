# cpulist

Utilities for parsing and emitting strings in the the `cpulist` format often used by Linux
utilities that work with processor IDs, memory region IDs and similar numeric hardware identifiers.

Example: `0,1,2-4,5-9:2,6-10:2`

The value is a comma-separated list of zero or more integers or integer ranges, where each item
is either:

* a single integer (e.g. `1`)
* a range of integers (e.g. `2-4`)
* a range of integers with a stride (step size) operator (e.g. `5-9:2` which is equivalent to `5,7,9`)

Whitespace or extra characters are not allowed anywhere in the string.

The identifiers in the list are of size `u32`.