//! Native action smoke fixture, compiled by tests/action.rs.
//! It records the real process boundary and renders minimal machine-readable artifacts;
//! no benchmark engine, storage service or GitHub operation runs in these adapter tests.

use std::env;
use std::fs::{self, OpenOptions};
use std::io::{Write as _, stdout};
use std::path::Path;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = env::args().skip(1).collect();
    let command = args.first().unwrap();
    let mut log = OpenOptions::new()
        .append(true)
        .create(true)
        .open(env::var_os("CBH_ACTION_FIXTURE_LOG").unwrap())
        .unwrap();
    writeln!(log, "{} {args:?}", env::current_dir().unwrap().display()).unwrap();
    assert_eq!(
        fs::canonicalize(env::current_dir().unwrap()).unwrap(),
        fs::canonicalize(env::var_os("CBH_ACTION_FIXTURE_CHECKOUT").unwrap()).unwrap()
    );
    for name in ["GITHUB_TOKEN", "GH_TOKEN"] {
        assert_eq!(
            env::var_os(name),
            env::var_os(format!("CBH_ACTION_FIXTURE_EXPECT_{name}"))
        );
    }
    if env::var("CBH_ACTION_FIXTURE_FAIL").as_deref() == Ok(command) {
        return ExitCode::FAILURE;
    }
    match command.as_str() {
        "collect" | "backfill" => {
            println!("fixture benchmark stdout");
            eprintln!("fixture benchmark stderr");
        }
        "machine-key" => {
            if env::var("CBH_ACTION_FIXTURE_FAIL").as_deref() == Ok("non-utf8") {
                // Invalid UTF-8 exercises the dedicated output decoder, not the fingerprint validator.
                stdout().write_all(&[255]).unwrap();
            } else {
                println!("0123456789ABCDEF");
            }
        }
        "analyze" => {
            let value = |flag: &str| args.iter().find_map(|arg| arg.strip_prefix(flag)).unwrap();
            let commit = value("--context=");
            let history = args.iter().any(|arg| arg == &format!("--base={commit}"));
            let mode = if history { "history" } else { "branch" };
            let report = format!(
                r#"{{"tip_commit":"{commit}","tip_dirty":false,"mode":"{mode}","outcome":"findings","notable":true,"regressions":2,"census":{{"coverage":"full","judged":2,"in_scope":2}}}}"#
            );
            fs::write(Path::new(value("--json=")), report).unwrap();
            fs::write(Path::new(value("--markdown=")), "Full tool report").unwrap();
            fs::write(
                Path::new(value("--markdown-summary=")),
                "Condensed tool summary",
            )
            .unwrap();
            fs::write(Path::new(value("--outcome=")), "findings\n").unwrap();
            println!("fixture analysis stdout");
        }
        _ => panic!(),
    }
    ExitCode::SUCCESS
}
