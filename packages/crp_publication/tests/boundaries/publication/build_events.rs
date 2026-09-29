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
    // Cargo can run multiple build scripts concurrently; append each complete record together.
    let event = format!(
        "{{\"operation\":\"build\",\"name\":\"{}\"}}\n",
        env::var("CARGO_PKG_NAME").unwrap()
    );
    events.write_all(event.as_bytes()).unwrap();
    println!("cargo::rerun-if-changed=build.rs");
}
