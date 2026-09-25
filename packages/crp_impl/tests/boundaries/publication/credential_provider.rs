//! Standalone fake Cargo credential provider for the local-registry integration test.
//!
//! Each invocation records the actual request and returns an uncached fixture credential.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Write};

fn main() {
    println!(r#"{{"v":[1]}}"#);
    io::stdout().flush().unwrap();
    let mut request = String::new();
    io::stdin().read_line(&mut request).unwrap();
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(env::var_os("CRP_PUBLICATION_EVENTS").unwrap())
        .unwrap()
        .write_all(request.as_bytes())
        .unwrap();
    println!(
        r#"{{"Ok":{{"kind":"get","token":"local-fixture","cache":"never","operation_independent":false}}}}"#
    );
}
