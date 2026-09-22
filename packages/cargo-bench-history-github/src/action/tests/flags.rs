use std::ffi::OsString;

use futures::executor::block_on;
use serde_json::json;

use crate::action::errors::InvalidInput;
use crate::action::execute::run_with;
use crate::action::flags::compiler_environment;
use crate::action::inputs::Inputs;
use crate::action::tests::fake::{FakeHost, FakePublisher};

#[test]
fn flags_append_to_effective_ambient_arguments_without_shell_parsing() {
    for (ambient, encoded, additional, expected) in [
        (None, None, "-C opt-level=3", "-C\u{1f}opt-level=3"),
        (Some(" \t "), None, "--cfg=extra", "--cfg=extra"),
        (
            Some(" \t-C opt-level=2  --cfg=ambient\t"),
            None,
            "  -Cllvm-args=-align-all-functions=6\t--cfg=extra ",
            "-C\u{1f}opt-level=2\u{1f}--cfg=ambient\u{1f}-Cllvm-args=-align-all-functions=6\u{1f}--cfg=extra",
        ),
        (
            Some("--cfg=ignored"),
            Some("-C\u{1f}link-arg=/LIBPATH:library directory\u{1f}--cfg=kept"),
            "-Cllvm-args=-align-all-functions=6",
            "-C\u{1f}link-arg=/LIBPATH:library directory\u{1f}--cfg=kept\u{1f}-Cllvm-args=-align-all-functions=6",
        ),
        (
            Some("-Cllvm-args=-align-all-functions=3"),
            None,
            "-Cllvm-args=-align-all-functions=6",
            "-Cllvm-args=-align-all-functions=3\u{1f}-Cllvm-args=-align-all-functions=6",
        ),
        (
            Some("--cfg=ignored"),
            Some(""),
            "--cfg=extra",
            "--cfg=extra",
        ),
        (
            None,
            Some("--cfg=kept\u{1f}"),
            "--cfg=extra",
            "--cfg=kept\u{1f}\u{1f}--cfg=extra",
        ),
        (
            Some("--cfg='ambient value'"),
            None,
            "--cfg=\"extra value\" $VARIABLE",
            "--cfg='ambient\u{1f}value'\u{1f}--cfg=\"extra\u{1f}value\"\u{1f}$VARIABLE",
        ),
    ] {
        let mut host = FakeHost::new(&json!({"command":"collect"}));
        for (name, value) in [("RUSTFLAGS", ambient), ("CARGO_ENCODED_RUSTFLAGS", encoded)] {
            if let Some(value) = value {
                host.env.insert(name.to_owned(), value.to_owned());
            }
        }
        let before = host.env.clone();
        assert_eq!(
            compiler_environment(Some(additional), &host).unwrap(),
            [(
                OsString::from("CARGO_ENCODED_RUSTFLAGS"),
                OsString::from(expected)
            )]
        );
        assert_eq!(host.env, before);
        assert_eq!(
            host.environment_reads.borrow().as_slice(),
            if encoded.is_some() {
                &["CARGO_ENCODED_RUSTFLAGS"][..]
            } else {
                &["CARGO_ENCODED_RUSTFLAGS", "RUSTFLAGS"][..]
            }
        );
    }
}

#[test]
fn collect_and_key_query_share_the_backfill_compiler_environment() {
    for command in ["collect", "backfill"] {
        let mut input = json!({
            "command":command, "rustflags":"-Cllvm-args=-align-all-functions=6"
        });
        if command == "backfill" {
            input["from"] = json!("older");
            input["to"] = json!("newer");
        }
        let mut host = FakeHost::new(&input);
        host.env.insert(
            "CARGO_ENCODED_RUSTFLAGS".to_owned(),
            "-C\u{1f}link-arg=path with spaces".to_owned(),
        );
        host.env
            .insert("PRIVATE_BUILD_HELPER".to_owned(), "untouched".to_owned());
        let before = host.env.clone();
        host.reply("");
        if command == "collect" {
            host.reply("0123456789abcdef");
        }
        block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
        let processes = host.processes.borrow();
        assert_eq!(processes.len(), if command == "collect" { 2 } else { 1 });
        for process in processes.iter() {
            assert_eq!(
                process.env,
                [(
                    OsString::from("CARGO_ENCODED_RUSTFLAGS"),
                    OsString::from(
                        "-C\u{1f}link-arg=path with spaces\u{1f}-Cllvm-args=-align-all-functions=6"
                    )
                )]
            );
        }
        assert_eq!(host.env, before);
        assert_eq!(
            host.environment_reads
                .borrow()
                .iter()
                .filter(|name| name.as_str() == "CARGO_ENCODED_RUSTFLAGS")
                .count(),
            1
        );
    }
}

#[test]
fn omitted_and_empty_input_preserve_inheritance_without_reading_flags() {
    for command in ["collect", "backfill"] {
        for additional in [None, Some("")] {
            let mut input = json!({"command":command});
            if let Some(additional) = additional {
                input["rustflags"] = json!(additional);
            }
            if command == "backfill" {
                input["from"] = json!("older");
                input["to"] = json!("newer");
            }
            let mut host = FakeHost::new(&input);
            host.unreadable_environment
                .extend(["RUSTFLAGS".to_owned(), "CARGO_ENCODED_RUSTFLAGS".to_owned()]);
            host.reply("");
            if command == "collect" {
                host.reply("0123456789abcdef");
            }
            block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap();
            assert!(
                host.processes
                    .borrow()
                    .iter()
                    .all(|process| process.env.is_empty())
            );
            assert!(
                !host
                    .environment_reads
                    .borrow()
                    .iter()
                    .any(|name| host.unreadable_environment.contains(name))
            );
        }
    }
}

#[test]
fn encoded_precedence_avoids_reading_unused_ordinary_flags() {
    let mut host = FakeHost::new(&json!({"command":"collect"}));
    host.env
        .insert("CARGO_ENCODED_RUSTFLAGS".to_owned(), String::new());
    host.unreadable_environment.insert("RUSTFLAGS".to_owned());
    assert_eq!(
        compiler_environment(Some("--cfg=extra"), &host).unwrap(),
        [(
            OsString::from("CARGO_ENCODED_RUSTFLAGS"),
            OsString::from("--cfg=extra")
        )]
    );
}

#[test]
fn unreadable_effective_flags_fail_before_starting_collection() {
    for name in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
        let mut host = FakeHost::new(&json!({"command":"collect","rustflags":"--cfg=extra"}));
        host.unreadable_environment.insert(name.to_owned());
        let error = block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
        assert_eq!(error.find_source::<InvalidInput>().unwrap().input, name);
        assert!(host.processes.borrow().is_empty());
        assert!(host.outputs.borrow().is_empty());
    }
}

#[test]
fn ordinary_ambient_argument_cannot_be_silently_split_by_encoded_separator() {
    let mut host = FakeHost::new(&json!({"command":"collect","rustflags":"--cfg=extra"}));
    host.env.insert(
        "RUSTFLAGS".to_owned(),
        "--cfg=kept\u{1f}argument".to_owned(),
    );
    let error = block_on(run_with(host.args(), &host, &FakePublisher::default())).unwrap_err();
    assert_eq!(
        error.find_source::<InvalidInput>().unwrap().input,
        "RUSTFLAGS"
    );
    assert!(host.processes.borrow().is_empty());
    assert!(host.outputs.borrow().is_empty());
}

#[test]
fn rustflags_require_measurement_command_and_representable_single_line() {
    for flags in [
        " ",
        "\t",
        "--cfg=a\n--cfg=b",
        "--cfg=a\r",
        "--cfg=a\0",
        "a\u{1f}b",
    ] {
        let input = json!({"command":"collect","rustflags":flags});
        let error = Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
        assert_eq!(
            error.find_source::<InvalidInput>().unwrap().input,
            "rustflags"
        );
    }
    for command in [
        "analyze-history",
        "analyze-pr",
        "publish-issue-preflight",
        "alert",
    ] {
        let input = json!({"command":command,"rustflags":"--cfg=extra"});
        let error = Inputs::parse(&serde_json::to_vec(&input).unwrap()).unwrap_err();
        assert_eq!(
            error.find_source::<InvalidInput>().unwrap().input,
            "rustflags"
        );
    }
}
