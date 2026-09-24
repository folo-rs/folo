//! Binary entry point for release-target-check.

use std::process::ExitCode;

use release_target_check::run;

// Executables own allocator selection; library dependencies must not impose it.
// Ref: docs/testing.md, "Executable allocators".
#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

fn main() -> ExitCode {
    run()
}
