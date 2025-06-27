Utilities for working with non-zero integers.

Currently this implements a shorthand macro for creating non-zero integers
from literals at compile time:

```rust
use std::num::NonZero;
use new_zealand::nz;

fn foo(x: NonZero<u32>) { println!("NonZero value: {x}"); }

foo(nz!(42));
```

More details in the [package documentation](https://docs.rs/new_zealand/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.