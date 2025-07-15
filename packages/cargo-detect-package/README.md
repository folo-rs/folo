A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.

This tool automatically detects which Cargo package a given file belongs to within a workspace, and then executes a subcommand with the appropriate package scope. It supports both cargo integration mode and environment variable mode for build tool integration.

## Usage

```bash
# Build the package containing src/lib.rs
cargo detect-package --path packages/events/src/lib.rs build
# Executes: cargo build -p events

# Test the package containing a specific test file  
cargo detect-package --path packages/many_cpus/tests/integration.rs test
# Executes: cargo test -p many_cpus

# Use with environment variable mode for non-cargo tools
cargo detect-package --path packages/events/src/lib.rs --via-env PACKAGE just test
# Executes: PACKAGE=events just test
```

The tool automatically adds `-p <package_name>` for specific packages or `--workspace` when no specific package is detected.

More details in the [package documentation](https://docs.rs/cargo-detect-package/).

This is part of the [Folo project](https://github.com/folo-rs/folo) that provides mechanisms for
high-performance hardware-aware programming in Rust.
