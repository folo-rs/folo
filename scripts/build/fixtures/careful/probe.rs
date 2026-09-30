//! Native child for CarefulRunner.Tests.ps1; records literal argv and build configuration.

use std::env;
use std::process;

fn main() {
    for argument in env::args().skip(1) {
        println!("argument={argument}");
    }
    for name in [
        "RUSTFLAGS",
        "CARGO_ENCODED_RUSTFLAGS",
        "RUSTDOCFLAGS",
        "CARGO_ENCODED_RUSTDOCFLAGS",
        "CARGO_TARGET_DIR",
        "FOLO_CAREFUL_BUILD_FLAGS",
    ] {
        println!("{name}={:?}", env::var_os(name));
    }
    process::exit(env::var("CAREFUL_PROBE_EXIT").unwrap().parse().unwrap());
}
