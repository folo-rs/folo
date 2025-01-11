# folo_hw

Hardware aware programming patterns for Rust apps.

# Dynamic changes in hardware configuration

The crate does not support detecting and responding to changes in the hardware environment at
runtime. For example, if the set of active processors changes at runtime (e.g. due to a change in
hardware resources or OS configuration) then this change might not be visible to the crate and
some threads may be executed on unexpected processors.

# Development environment setup (Windows)

Prerequisites:

* Windows 11
* Visual Studio 2022 with workload "Desktop development with C++"
* Visual Studio Code with extensions:
    * rust-analyzer
    * C/C++
    * WSL
* Rust development tools (see `rust-toolchain.toml` for version) with additional tools:
    * `cargo install cargo-nextest --locked`

Setup:

1. In repo directory, execute `git config --local include.path ./.gitconfig` to attach the repo-specific Git configuration.

Validation:

1. Open repo directory in Visual Studio code.
1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`

# Development environment setup (Linux)

Prerequisites:

* Ubuntu 24 installed in WSL
* Visual Studio Code (remote) with extensions:
    
* Rust development tools (see `rust-toolchain.toml` for version) with additional tools:
    * `cargo install cargo-nextest --locked`

Setup:

1. Navigate to repo shared with Windows host (under `/mnt/c/`)
1. If first time setup, execute `git config --global credential.helper "/mnt/c/Program\ Files/Git/mingw64/bin/git-credential-manager.exe"` to set the correct Git authentication flow.
1. Open Visual Studio code via `code .`
1. If first time setup, install required Visual Studio Code extensions:
    * rust-analyzer
    * C/C++

Validation:

1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`