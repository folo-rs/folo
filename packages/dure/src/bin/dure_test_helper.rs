//! Child process used by `dure` Windows integration tests.

#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(coverage_nightly, coverage(off))]

use std::io::{self, Read, Write};

fn wait_for_byte() {
    let mut buf = [0_u8; 1];
    _ = io::stdin().read(&mut buf);
}

fn print_console_status() {
    let status = if stdin_is_console() {
        "console"
    } else {
        "pipes"
    };
    // Also write a cwd file so integration tests can observe the result
    // without scraping ConPTY VT output.
    std::fs::write("console-status.txt", status).expect("write console status");
    println!("{status}");
    io::stdout().flush().expect("flush stdout");
}

fn stdin_is_console() -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::System::Console::{
            CONSOLE_MODE, GetConsoleMode, GetStdHandle, STD_OUTPUT_HANDLE,
        };

        // SAFETY: `GetStdHandle` returns a process-lifetime handle that this
        // process does not own or close.
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let Ok(handle) = handle else {
            return false;
        };
        let mut mode = CONSOLE_MODE::default();
        // SAFETY: `handle` is a standard handle; `mode` is a stack value that
        // outlives the call.
        unsafe { GetConsoleMode(handle, &raw mut mode) }.is_ok()
    }
    #[cfg(not(windows))]
    {
        false
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
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
            let mut buf = [0_u8; 1];
            _ = io::stdin().read(&mut buf);
        }
        Some("exit") => {
            let code = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            std::process::exit(code);
        }
        Some("has-console") => {
            print_console_status();
        }
        Some("wait-has-console") => {
            wait_for_byte();
            print_console_status();
        }
        Some("wait-exit") => {
            let code = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
            wait_for_byte();
            std::process::exit(code);
        }
        _ => {
            eprintln!(
                "usage: dure-test-helper echo-line | print-and-wait | exit [code] | \
                 has-console | wait-has-console | wait-exit [code]"
            );
            std::process::exit(2);
        }
    }
}
