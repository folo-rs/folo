//! Windows integration tests that drive `dure` inside a test-owned `ConPTY`.

#![cfg(all(windows, feature = "private-test-util"))]

use std::fs;
use std::path::PathBuf;

use dure::test_support::ConsoleProcess;
use tempfile::TempDir;
use testing::with_watchdog;

fn dure_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_dure"))
}

fn helper_exe() -> PathBuf {
    PathBuf::from(dure_test_helper::binary_path())
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

/// Text with all whitespace removed.
///
/// `ConPTY` wraps output at the window width and can break a line in the middle
/// of a word, so ignoring whitespace is the only stable way to look for a
/// phrase in console output.
fn without_whitespace(text: &str) -> String {
    text.chars().filter(|ch| !ch.is_whitespace()).collect()
}

fn collect_until(console: &ConsoleProcess, needle: &str) -> String {
    let wanted = without_whitespace(needle);
    let mut collected = Vec::new();
    loop {
        collected.extend(console.read_output());
        let text = visible_text(&collected);
        if without_whitespace(&text).contains(&wanted) {
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
        let report = fs::read_to_string(dir.path().join("console-status.txt"))
            .expect("helper console status file");
        assert_eq!(report, "console");
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn a_dropped_client_leaves_the_app_resumable() {
    with_watchdog(|| {
        let dir = TempDir::new().unwrap();
        let helper = helper_exe();
        let run_args = vec![
            "--store-root".to_string(),
            dir.path().display().to_string(),
            "run".to_string(),
            "--".to_string(),
            helper.display().to_string(),
            "wait-exit".to_string(),
            SAMPLE_NONZERO_EXIT.to_string(),
        ];
        let client = ConsoleProcess::spawn(&dure_exe(), &run_args, dir.path());
        _ = collect_until(&client, "session");
        // Models the SSH connection dropping: the client dies where it stands,
        // without a chance to tell the supervisor anything.
        drop(client);

        let resume_args = vec![
            "--store-root".to_string(),
            dir.path().display().to_string(),
            "resume".to_string(),
        ];
        let resumed = ConsoleProcess::spawn(&dure_exe(), &resume_args, dir.path());
        _ = collect_until(&resumed, "session");
        // The app was blocked on console input across the disconnect. Reaching
        // it now, and seeing its own exit status arrive, is what makes the
        // resumed session interactive rather than merely alive.
        resumed.write_input(b"x\r\n");
        let status = resumed.wait();
        assert_eq!(status, SAMPLE_NONZERO_EXIT);
    });
}

#[cfg_attr(miri, ignore)]
#[test]
fn run_refuses_a_launcher_that_forbids_breakaway() {
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
        // Models a wrapper such as `cargo run`, whose job would kill the
        // supervisor along with the client it launched.
        let client = ConsoleProcess::spawn_confined(&dure_exe(), &args, dir.path());
        let output = collect_until(&client, "forbids breakaway");
        let status = client.wait();
        assert_ne!(status, 0, "client output: {output:?}");
        assert!(
            !output.contains("session "),
            "a refused launch must not report a session, got {output:?}"
        );
        // Nothing may be left behind for `resume` or `list` to find.
        let records = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert!(records.is_empty(), "store must stay empty, got {records:?}");
    });
}
