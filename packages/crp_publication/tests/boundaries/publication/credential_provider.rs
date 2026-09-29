//! Links the production provider into the local-registry Cargo boundary fixture.
//!
//! Requests are recorded without rewriting them. Cargo's own local crates.io routing retains
//! the canonical credential identity; decoding, authority checks and leases remain production code.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]
#![allow(
    clippy::unwrap_used,
    reason = "This executable is an integration-test fixture"
)]

use std::env;
use std::fs::OpenOptions;
use std::io::{self, BufRead, Cursor, Read, Write};
use std::sync::Arc;

use crp_publication::PublicationOutput;
use crp_publication::publication::credentials::provide;
use ohno::AppError;

::testing::set_allocator!();

fn main() -> Result<(), AppError> {
    assert_eq!(env::args().skip(1).collect::<Vec<_>>(), ["--cargo-plugin"]);
    provide(
        &mut RecordingInput { request: None },
        &mut io::stdout().lock(),
        &PublicationOutput::new("1.0.0", false, Arc::new(crp_diag::Discard)),
    )
}

/// Defers reading until production sends its hello, preserving Cargo's pipe handshake.
struct RecordingInput {
    request: Option<Cursor<Vec<u8>>>,
}

impl BufRead for RecordingInput {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.request.is_none() {
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(env::var_os("CRP_PUBLICATION_EVENTS").unwrap())?
                .write_all(line.as_bytes())?;
            self.request = Some(Cursor::new(line.into_bytes()));
        }
        self.request.as_mut().unwrap().fill_buf()
    }

    fn consume(&mut self, amount: usize) {
        self.request.as_mut().unwrap().consume(amount);
    }
}

impl Read for RecordingInput {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let count = self.fill_buf()?.read(buf)?;
        self.consume(count);
        Ok(count)
    }
}
