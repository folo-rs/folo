use std::fs;
use std::path::PathBuf;

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::cache::entries;
use crate::compatibility::{CHECKER_WATCHDOG, checker_command, read_outcome};
use crate::compatibility_cache::{reused, same_evidence, success};
use crate::fixture::{Fixture, write_package};

#[test]
#[cfg_attr(
    miri,
    ignore = "Replaces native cache entries between independent CLI invocations"
)]
fn no_target_checks_admit_source_with_missing_disabled_or_invalid_cache() {
    testing::with_watchdog_timeout(CHECKER_WATCHDOG, || {
        let fixture = Fixture::new("");
        write_package(&fixture, "library", "1.0.0", "");
        fixture.commit("unchanged public library");
        let evidence = TempDir::new().unwrap();
        let storage = evidence.path().join("cache");
        let prepared = evidence.path().join("prepared");
        success(
            checker_command()
                .current_dir(fixture.path())
                .args(["prepare", "--release-history", "HEAD", "--cache"])
                .arg(&storage)
                .arg("--output")
                .arg(&prepared)
                .output()
                .unwrap(),
        );
        let expected = fs::read(prepared.join("report.json")).unwrap();
        let mut baseline: Option<PathBuf> = None;
        for state in [
            "warm",
            "corrupt",
            "format",
            "revision",
            "producer",
            "deleted",
            "unavailable",
            "disabled",
        ] {
            if state == "deleted" || state == "unavailable" {
                fs::remove_dir_all(&storage).unwrap();
                if state == "unavailable" {
                    fs::create_dir_all(&storage).unwrap();
                    fs::write(storage.join("classification-decisions"), "not a directory").unwrap();
                }
            } else if ["corrupt", "format", "revision", "producer"].contains(&state) {
                for path in entries(&storage, "classification-decisions") {
                    let mut entry: Value =
                        serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
                    match state {
                        "corrupt" => {
                            *entry.get_mut("checksum").unwrap() = json!("invalid integrity");
                        }
                        "format" | "revision" => *entry.get_mut(state).unwrap() = json!(u32::MAX),
                        "producer" => {
                            let key: String =
                                serde_json::from_str(entry.get("key").unwrap().as_str().unwrap())
                                    .unwrap();
                            let mut key: Value = serde_json::from_str(&key).unwrap();
                            *key.get_mut(0).unwrap() = json!("incompatible-producer");
                            *entry.get_mut("key").unwrap() =
                                json!(serde_json::to_string(&key.to_string()).unwrap());
                        }
                        _ => unreachable!(),
                    }
                    fs::write(path, serde_json::to_vec(&entry).unwrap()).unwrap();
                }
            }
            let output = evidence.path().join(state);
            let calls = evidence.path().join(format!("{state}.calls"));
            let mut command = checker_command();
            command
                .current_dir(fixture.path())
                .args(["check-compatibility", "--verbose", "--prepared"])
                .arg(prepared.join("prepared.json"))
                .arg("--output")
                .arg(&output)
                .env("CRP_FIXTURE_CALLS", &calls)
                .env("CRP_FIXTURE_SCENARIO", "identity-failure");
            if state == "disabled" {
                command.arg("--no-cache");
            } else {
                command.arg("--cache").arg(&storage);
            }
            let result = success(command.output().unwrap());
            let diagnostics = String::from_utf8_lossy(&result.stderr);
            if state == "warm" {
                reused(&result);
            } else {
                assert!(!diagnostics.contains("reusing classification decisions"));
                if state != "disabled" {
                    assert!(
                        diagnostics.contains("computed classification decisions"),
                        "{diagnostics}"
                    );
                }
            }
            if state == "corrupt" || state == "unavailable" {
                assert!(diagnostics.contains("continuing with fresh observations"));
            }
            assert!(!calls.exists());
            assert_eq!(fs::read(output.join("report.json")).unwrap(), expected);
            let outcome = read_outcome(&output);
            assert_eq!(outcome.get("packages").unwrap(), &json!([]));
            assert_eq!(outcome.get("completed").unwrap(), true);
            if let Some(baseline) = &baseline {
                same_evidence(baseline, &output);
            } else {
                baseline = Some(output);
            }
            if state == "unavailable" {
                fs::remove_dir_all(&storage).unwrap();
            }
        }
        assert!(!storage.exists());
    });
}
