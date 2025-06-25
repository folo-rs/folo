Utilities for working with non-zero integers.

Currently this implements a shorthand macro for creating non-zero integers
from literals at compile time:

```
use std::num::NonZero;
use new_zealand::nz;

fn foo(x: NonZero<u32>) { println!("NonZero value: {x}"); }

foo(nz!(42));
```