//! Records Cargo package verification in the local-registry integration fixture.

use std::env;
use std::fs::OpenOptions;
use std::io::Write;

fn main() {
    let mut events = OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var_os("CRP_PUBLICATION_EVENTS").unwrap())
        .unwrap();
    writeln!(
        events,
        r#"{{"operation":"build","name":"{}"}}"#,
        env::var("CARGO_PKG_NAME").unwrap()
    )
    .unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}
