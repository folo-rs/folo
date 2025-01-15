# folo_hw

Hardware aware programming patterns for Rust apps.

# Dynamic changes in hardware configuration

The crate does not support detecting and responding to changes in the hardware environment at
runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
hardware resources or OS configuration) then this change might not be visible to the crate and
some threads may be executed on unexpected processors.

# Development environment setup

See [DEVELOPMENT.md](DEVELOPMENT.md).