//! Entry point for the repository's inexpensive action-pairing relevance check.

use std::process::ExitCode;

// Executables own allocator selection; library dependencies must not impose it.
// Ref: docs/testing.md, "Executable allocators".
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    benchmark_action_check::run()
}
