//! Release workflow entry point.

use std::process::ExitCode;

#[cfg(not(miri))]
use mimalloc::MiMalloc;

#[cfg(not(miri))]
#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    release_binaries::run()
}
