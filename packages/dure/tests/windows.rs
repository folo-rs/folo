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
    let mut path = PathBuf::from(env!("CARGO_BIN_EXE_dure"));
    path.set_file_name("dure-test-helper.exe");
    path
}

/// Drop CSI/OSC sequences so tests can match app text that `ConPTY` emits with
/// intervening cursor commands (for example `session` then the id).
fn visible_text(bytes: &[u8]) -> String {
    let raw = String::from_utf8_lossy(bytes);
    let mut chars = raw.chars();
    let mut text = String::new();
    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
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
                    if next == '\u{7}' {
                        break;
                    }
                }
            }
            Some(_) | None => {}
        }
    }
    text
}

/// Ceiling on spinning until the supervisor deletes the session record after
/// the client has exited. Local filesystem delete is immediate; this bound
/// exists so a stuck record fails the test instead of hanging under mutation
/// testing.
const RECORD_GC_SPIN_LIMIT: u32 = 1_000_000;

fn collect_until(client: &ConsoleProcess, needle: &str) -> String {
    let mut collected = Vec::new();
    loop {
        collected.extend(client.read_output());
        let text = visible_text(&collected);
        if text.contains(needle) {
            return text;
        }
    }
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_exit_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["exit".to_string(), "7".to_string()];
        let client = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        let status = client.wait();
        assert_eq!(status, 7);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_has_console_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["has-console".to_string()];
        let client = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        let output = collect_until(&client, "console");
        let status = client.wait();
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
            "7".to_string(),
        ];
        let client = ConsoleProcess::spawn(&dure_exe(), &args, dir.path());
        let attached = collect_until(&client, "session");
        client.write_input(b"x\r\n");
        let status = client.wait();
        assert_eq!(status, 7, "client output: {attached:?}");
        // The client process can exit as soon as it receives `AppExited`, while
        // the supervisor still deletes the record.
        let mut gone = false;
        for _ in 0..RECORD_GC_SPIN_LIMIT {
            if dir.path().read_dir().unwrap().next().is_none() {
                gone = true;
                break;
            }
        }
        assert!(gone, "session records must be gone after the app exits");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn helper_echo_via_conpty() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let args = vec!["echo-line".to_string()];
        let client = ConsoleProcess::spawn(&helper_exe(), &args, dir.path());
        client.write_input(b"hello\r\n");
        let output = collect_until(&client, "hello");
        let status = client.wait();
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
