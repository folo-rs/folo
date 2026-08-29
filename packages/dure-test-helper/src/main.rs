//! Child process driven by the `dure` Windows integration tests.
//!
//! Each subcommand parks the process in one observable state — waiting on
//! console input, reporting whether it sees a console, exiting with a chosen
//! status — so a test can drive the state a scenario needs and assert on it.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

#[cfg(windows)]
use std::io::{self, IsTerminal, Read, Write};
#[cfg(windows)]
use std::{env, fs, process};

/// The helper serves Windows integration tests only, so on other platforms the
/// binary is an empty stub, matching `dure` itself
/// (`dure/docs/implementation.md`, "Platform gate").
#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("echo-line") => {
            let mut line = String::new();
            io::stdin().read_line(&mut line).expect("read stdin");
            print!("{line}");
            io::stdout().flush().expect("flush stdout");
        }
        Some("print-and-wait") => {
            println!("ready");
            io::stdout().flush().expect("flush stdout");
            wait_for_byte();
        }
        Some("exit") => {
            process::exit(exit_code(&args));
        }
        Some("has-console") => {
            print_console_status();
        }
        Some("wait-has-console") => {
            wait_for_byte();
            print_console_status();
        }
        Some("wait-exit") => {
            wait_for_byte();
            process::exit(exit_code(&args));
        }
        _ => {
            eprintln!(
                "usage: dure-test-helper echo-line | print-and-wait | exit [code] | \
                 has-console | wait-has-console | wait-exit [code]"
            );
            process::exit(2);
        }
    }
}

/// The status the `exit` and `wait-exit` subcommands terminate with.
#[cfg(windows)]
fn exit_code(args: &[String]) -> i32 {
    args.get(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(0)
}

/// Blocks until console input arrives, modelling an app parked on the user.
#[cfg(windows)]
fn wait_for_byte() {
    let mut buf = [0_u8; 1];
    _ = io::stdin().read(&mut buf);
}

/// Reports whether this process was given a real console or redirected pipes.
///
/// The result also goes to a file in the working directory so a test can read
/// it without scraping pseudoconsole output for text the console host is free
/// to reflow.
#[cfg(windows)]
fn print_console_status() {
    let status = if io::stdin().is_terminal() {
        "console"
    } else {
        "pipes"
    };
    fs::write("console-status.txt", status).expect("write console status");
    println!("{status}");
    io::stdout().flush().expect("flush stdout");
}
