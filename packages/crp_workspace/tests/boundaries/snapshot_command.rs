use std::fs;
use std::io::Error;
use std::string::FromUtf8Error;

use crp_workspace::snapshot_command::{capture, git};
use tempfile::TempDir;

use crate::with_io_test;

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn captures_successful_output() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        let output = capture("git", ["--version"], directory.path()).unwrap();
        assert!(!output.is_empty());
        assert!(
            String::from_utf8(output)
                .unwrap()
                .starts_with("git version ")
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn preserves_unsuccessful_exit_as_a_process_failure() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        let error = capture("git", ["--not-a-git-option"], directory.path()).unwrap_err();
        assert!(error.find_source::<Error>().is_none());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Starts a subprocess with an absent working directory")]
fn preserves_process_start_failure() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        let error = capture("git", ["--version"], &directory.path().join("absent")).unwrap_err();
        assert!(error.find_source::<Error>().is_some());
    });
}

#[test]
#[cfg_attr(miri, ignore = "Executes Git against filesystem fixtures")]
fn rejects_non_utf8_text_output() {
    with_io_test(|| {
        let directory = TempDir::new().unwrap();
        // Git config values are byte strings; this is not valid UTF-8 text.
        fs::write(
            directory.path().join("fixture.config"),
            b"[probe]\nvalue = \xff\n",
        )
        .unwrap();
        let error = git(
            ["config", "--file", "fixture.config", "--get", "probe.value"],
            directory.path(),
        )
        .unwrap_err();
        assert!(error.find_source::<FromUtf8Error>().is_some());
    });
}
