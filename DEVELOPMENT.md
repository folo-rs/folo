# The basics

This is a multiplatform project supporting both Windows and Linux. Development of the Linux
functionality takes place in a Windows Subsystem for Linux (WSL) virtual machine.

See `rust-toolchain.toml` for the required stable Rust toolchain version. The `nightly` toolchain
is also required for some development tooling.

# Development environment setup (Windows)

Prerequisites:

* Windows 11
* Visual Studio 2022 with workload "Desktop development with C++"
* Visual Studio Code with extensions:
    * C/C++
    * rust-analyzer
    * vscode-just
    * WSL
* PowerShell 7
* `rustup toolchain install` to install Rust development tools based on `rust-toolchain.toml`
* `cargo install just`
* (Only if publishing releases) GitHub CLI + `gh auth login`

Setup:

1. Clone the repo to a directory of your choosing.
1. Open a terminal in the repo root.
1. Execute `git config --local include.path ./.gitconfig` to attach the repo-specific Git configuration.
1. Execute `just install-tools` to install development tools.

Validation:

1. Open repo directory in Visual Studio code.
1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`
1. Execute `just validate-local` in terminal.

# Development environment setup (Linux)

Prerequisites:

* Ubuntu 24 installed in WSL
* `sudo apt install -y git git-lfs build-essential cmake gcc make curl libssl-dev pkg-config`
* Git LFS setup: `git lfs install`
* [PowerShell 7](https://learn.microsoft.com/en-us/powershell/scripting/install/install-ubuntu?view=powershell-7.5):
  ```bash
  # Download and install Microsoft package repository
  wget -q "https://packages.microsoft.com/config/ubuntu/$(lsb_release -rs)/packages-microsoft-prod.deb"
  sudo dpkg -i packages-microsoft-prod.deb
  sudo apt update
  sudo apt install -y powershell
  ```
* `rustup toolchain install` to install Rust development tools based on `rust-toolchain.toml`
* `cargo install just`
* If first time Git setup, execute `git config --global credential.helper "/mnt/c/Program\ Files/Git/mingw64/bin/git-credential-manager.exe"` to setup authentication flow

Setup:

1. Navigate to repo shared with Windows host (under `/mnt/c/`). Do not create a separate clone of the repo for Linux.
1. Execute `just install-tools` to install development tools.
1. Open Visual Studio code via `code .`
1. If first time setup, install required Visual Studio Code extensions:
    * C/C++
    * rust-analyzer

Validation:

1. Execute from task palette (F1):
    * `Tasks: Run Build Task`
    * `Tasks: Run Test Task`
1. Execute `just validate-local` in terminal.
