# cargo-detect-package

A Cargo tool to detect the package that a file belongs to, passing the package name to a subcommand.

This tool automatically detects which Cargo package a given file belongs to within a workspace,
and then executes a subcommand with the appropriate package scope. It supports two operating modes:
cargo integration mode (default) and environment variable mode.

## Usage

Install via `cargo install cargo-detect-package` and then:

```text
cargo detect-package --path <PATH> [--via-env <ENV_VAR>] [--outside-package <ACTION>] <SUBCOMMAND>...
```

### Arguments

- `--path <PATH>`: Path to the file for which to detect the package
- `--via-env <ENV_VAR>`: Optional. Pass the package name via environment variable instead of cargo arguments
- `--outside-package <ACTION>`: Optional. Action to take when path is not in any package (workspace, ignore, error). Defaults to workspace.
- `<SUBCOMMAND>...`: The command to execute with the detected package information

## Operating Modes

### Cargo Integration Mode (Default)

In this mode, the tool automatically adds the appropriate cargo package arguments to the subcommand:
- If a specific package is detected: adds `-p <package_name>`
- If no package is detected (workspace scope): adds `--workspace`

#### Examples

```bash
# Build the package containing src/lib.rs
cargo detect-package --path packages/events/src/lib.rs build
# Prints: Detected package: events
# Executes: cargo build -p events

# Test the package containing a specific test file
cargo detect-package --path packages/many_cpus/tests/integration.rs test
# Prints: Detected package: many_cpus
# Executes: cargo test -p many_cpus

# Check a file in the workspace root (falls back to workspace by default)
cargo detect-package --path README.md check
# Prints: Path is not in any package, using workspace scope
# Executes: cargo check --workspace

# Error when a file is not in any package
cargo detect-package --path README.md --outside-package error check
# Prints: Error: Path is not in any package
# Exits with code 1, does not execute subcommand

# Ignore when a file is not in any package
cargo detect-package --path README.md --outside-package ignore check
# Prints: Path is not in any package, ignoring as requested
# Exits with code 0, does not execute subcommand

# Run clippy with additional arguments
cargo detect-package --path packages/events/src/lib.rs clippy -- -D warnings
# Prints: Detected package: events
# Executes: cargo clippy -p events -- -D warnings
```

#### Visual Studio Code Integration

```json
{
    "label": "rust: build (current package)",
    "type": "cargo",
    "command": "detect-package",
    "args": [
        "--path",
        "${relativeFileDirname}",
        "build",
        "--all-features",
        "--all-targets"
    ],
    "group": {
        "kind": "build",
        "isDefault": true
    },
    "problemMatcher": [
        "$rustc"
    ]
}
```

### Environment Variable Mode

In this mode, the tool sets an environment variable with the detected package name
and executes a non-cargo command. This is useful for integration with build tools
like `just`, `make`, or custom scripts.

#### Examples

```bash
# Use with just command runner
cargo detect-package --path packages/events/src/lib.rs --via-env package just build
# Prints: Detected package: events
# Executes: just build (with package=events environment variable)

# Use with custom script
cargo detect-package --path packages/many_cpus/src/lib.rs --via-env PKG_NAME ./build.sh
# Prints: Detected package: many_cpus
# Executes: ./build.sh (with PKG_NAME=many_cpus environment variable)

# Workspace scope with environment variable (default behavior)
cargo detect-package --path README.md --via-env package just test
# Prints: Path is not in any package, using workspace scope
# Executes: just test (no environment variable set, allowing just to handle workspace scope)

# Error when not in package with environment variable mode
cargo detect-package --path README.md --via-env package --outside-package error just test
# Prints: Error: Path is not in any package
# Exits with code 1, does not execute subcommand
```

## Workspace Validation

The tool requires that both the current directory and target path are within the same Cargo workspace.
Cross-workspace operations are rejected with an error.

## Fallback Behavior

The tool's behavior when a file is not within any package is configurable via the `--outside-package` flag:

- `workspace` (default): Falls back to workspace scope and executes the subcommand with `--workspace` flag or no environment variable
- `ignore`: Does not execute the subcommand and exits with success (code 0)
- `error`: Does not execute the subcommand and exits with failure (code 1)

Common scenarios that trigger outside-package behavior:
- Non-existent files or directories
- Files outside the workspace  
- Files in the workspace root that don't belong to any specific package
- Invalid or missing package configuration
