# folo_hw

Hardware aware programming patterns for Rust apps.

# Dynamic changes in hardware configuration

The crate does not support detecting and responding to changes in the hardware environment at
runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
hardware resources or OS configuration) then this change might not be visible to the crate and
some threads may be executed on unexpected processors.

# Development environment setup

Windows prerequisites:

* Windows 11
* Visual Studio 2022 with workload "Desktop development with C++"
* Visual Studio Code with extensions:
    * rust-analyzer
    * C/C++
    * WSL
* Rust development tools (see `rust-toolchain.toml` for version)

Linux prerequisites:

* Ubuntu 24 installed in WSL
* Visual Studio Code (remote) with extensions:
    * rust-analyzer
    * C/C++
* Rust development tools (see `rust-toolchain.toml` for version)

Setup (Windows):

1. In repo directory, execute `git config --local include.path ./.gitconfig` to attach the repo-specific Git configuration.

Validation (Windows):

1. Open repo directory in Visual Studio code.
1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`

Setup (Linux):

1. Navigate to repo shared with Windows host (under `/mnt/c/`)
1. Open Visual Studio code via `code .`
    * Install required extensions if this is the first time you run Visual Studio Code under Linux.
1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`