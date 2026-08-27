//! Windows integration tests that drive `dure` inside a test-owned `ConPTY`.

#![cfg(all(windows, feature = "private-test-util"))]

use std::path::PathBuf;

use dure::test_support::ConsoleProcess;
use tempfile::TempDir;
use testing::with_watchdog;

fn dure_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dure"))
}

fn helper_exe() -> PathBuf {
    // Cargo writes every binary from this package next to `CARGO_BIN_EXE_dure`.
    // The helper is a second `[[bin]]` in the same package, so replacing the
    // file name is the path Cargo emits.
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_dure"));
    path.set_file_name("dure-test-helper.exe");
    path
}

/// Introducer for CSI, OSC, and other ECMA-48 sequences.
const ESC: char = '\u{1b}';
/// OSC terminator used by `ConPTY` window-title sequences.
const BEL: char = '\u{7}';

/// Strip the control sequences `ConPTY` typically injects around app text.
///
/// ESC introduces a sequence. CSI (`ESC [`) runs until an ASCII letter, the
/// ECMA-48 final byte for SGR and cursor commands. OSC (`ESC ]`) runs until
/// BEL, the terminator `ConPTY` uses for window-title sequences. Any other ESC
/// form consumes one following character. This recovers app text from `ConPTY`
/// cursor noise; it is not a full VT parser.
fn visible_text(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut chars = raw.chars();
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        if ch != ESC {
            text.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            Some(']') => {
                for next in chars.by_ref() {
                    if next == BEL {
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    text
}

fn collect_until(console: &ConsoleProcess, needle: &str) -> String {
    let mut collected = Vec::new();
    loop {
        collected.extend(console.read_output());
        let text = visible_text(&collected);
        if text.contains(needle) {
            return text;
        }
    }
}

/// Arbitrary nonzero status used to prove forwarding, not a special value.
const SAMPLE_NONZERO_EXIT: i32 = 7;

#[cfg_attr(miri, ignore)]
#[test]
fn helper_exit_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["exit".to_string(), SAMPLE_NONZERO_EXIT.to_string()];
        let helper = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        let status = helper.wait();
        assert_eq!(status, SAMPLE_NONZERO_EXIT);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_has_console_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["has-console".to_string()];
        let helper = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        let output = collect_until(&helper, "console");
        let status = helper.wait();
        assert_eq!(status, 0);
        assert!(
            output.contains("console"),
            "helper must report a console, got {output:?}"
        );
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn run_helper_exit_status() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let helper = helper_exe();
        let args = vec![
            "--store-root".to_string(),
            dir.path().display().to_string(),
            "run".to_string(),
            "--".to_string(),
            helper.display().to_string(),
            "wait-exit".to_string(),
            SAMPLE_NONZERO_EXIT.to_string(),
        ];
        let client = ConsoleProcess::spawn(&dure_exe(), &args, dir.path());
        let attached = collect_until(&client, "session");
        // The helper's console is in line-input mode, so a lone character is
        // not delivered to `stdin.read` until a newline arrives.
        client.write_input(b"x\r\n");
        let status = client.wait();
        assert_eq!(status, SAMPLE_NONZERO_EXIT, "client output: {attached:?}");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_echo_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["echo-line".to_string()];
        let helper = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        helper.write_input(b"hello\r\n");
        let output = collect_until(&helper, "hello");
        let status = helper.wait();
        assert_eq!(status, 0, "helper output: {output:?}");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_sees_a_console() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let helper = helper_exe();
        let args = vec![
            "--store-root".to_string(),
            dir.path().display().to_string(),
            "run".to_string(),
            "--".to_string(),
            helper.display().to_string(),
            "wait-has-console".to_string(),
        ];
        let client = ConsoleProcess::spawn(&dure_exe(), &args, dir.path());
        // ConPTY may emit `session` and the id with intervening cursor sequences
        // instead of a literal `session `.
        _ = collect_until(&client, "session");
        // The helper's console is in line-input mode, so a lone character is
        // not delivered to `stdin.read` until a newline arrives.
        client.write_input(b"x\r\n");
        let status = client.wait();
        assert_eq!(status, 0);
        let report = std::fs::read_to_string(dir.path().join("console-status.txt"))
            .expect("helper console status file");
        assert_eq!(report, "console");
    });
}
