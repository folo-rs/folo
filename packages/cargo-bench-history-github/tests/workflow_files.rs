//! Native CLI boundaries for offline receipt creation and validated report outputs.
//! Tests create only owned, repository-local fixture directories and never use GitHub credentials.

use std::env::consts::OS;
#[cfg(unix)]
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{fs, thread};

#[cfg(feature = "private-test-util")]
use cargo_bench_history_github::{__private, Cli};
#[cfg(feature = "private-test-util")]
use clap::Parser as _;
#[cfg(feature = "private-test-util")]
use ohno::AppError;
use serde_json::{Value, json};

/// Owns one isolated native fixture without depending on a real-time clock or system temp.
struct Fixture(PathBuf);

impl Fixture {
    #[expect(
        clippy::create_dir,
        reason = "A fixture collision must fail, not adopt and delete old data."
    )]
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("workflow-evidence-tests")
            .join(format!(
                "{}-{}-{}",
                OS,
                process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        fs::create_dir_all(root.parent().unwrap()).unwrap();
        fs::create_dir(&root).unwrap();
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-github"));
        command.current_dir(&self.0);
        for name in ["GITHUB_TOKEN", "GH_TOKEN", "GITHUB_REPOSITORY"] {
            command.env_remove(name);
        }
        command
    }

    fn receipt(&self, platform: &str, head: &str, output: &Path) -> Output {
        self.command()
            .args([
                "--repository",
                "folo-rs/folo",
                "--instance",
                "folo",
                "collection-receipt",
                "--run-id",
                "42",
                "--run-attempt",
                "2",
                "--head",
                head,
                "--platform",
                platform,
                "--machine-key-file",
            ])
            .arg(self.path("machine-key.txt"))
            .arg("--file")
            .arg(output)
            .output()
            .unwrap()
    }

    fn inspect(&self, head: &str, completed: &str) -> Output {
        self.command()
            .args(["inspect-report", "--report-file"])
            .arg(self.path("report.json"))
            .args([
                "--analyzed-sha",
                head,
                "--expected-platforms",
                "linux,windows",
                "--completed-platforms",
                completed,
                "--github-output",
            ])
            .arg(self.path("github-output"))
            .output()
            .unwrap()
    }

    #[cfg(feature = "private-test-util")]
    fn artifact(&self, platform: &str, key: &str) -> PathBuf {
        let artifact = self.path("receipts").join(platform);
        fs::write(self.path("machine-key.txt"), key).unwrap();
        let output = self.receipt(platform, &head(), &artifact.join("receipt.json"));
        assert!(output.status.success(), "{output:?}");
        artifact
    }

    #[cfg(feature = "private-test-util")]
    fn prepare(&self, jobs: &Value) -> Result<(), AppError> {
        self.prepare_to(jobs, &self.path("keys"), &self.path("github-output"))
    }

    #[cfg(feature = "private-test-util")]
    fn prepare_to(&self, jobs: &Value, keys: &Path, output: &Path) -> Result<(), AppError> {
        let mut command = self.command();
        command
            .args([
                "--repository",
                "folo-rs/folo",
                "--instance",
                "folo",
                "prepare-analysis",
                "--run-id",
                "42",
                "--head",
                &head(),
                "--expected-platforms",
                "linux,windows",
                "--receipts-dir",
            ])
            .arg(self.path("receipts"))
            .arg("--machine-key-dir")
            .arg(keys)
            .arg("--github-output")
            .arg(output);
        let cli =
            Cli::try_parse_from(std::iter::once(command.get_program()).chain(command.get_args()))
                .unwrap();
        __private::prepare_analysis(cli, &jobs.to_string())
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let result = fs::remove_dir_all(&self.0);
        if !thread::panicking() {
            result.unwrap();
        }
    }
}

fn head() -> String {
    "a".repeat(40)
}

fn report(mode: &str) -> Value {
    json!({
        "mode": mode, "tip_commit": head(), "tip_dirty": false,
        "outcome": "clean", "notable": false,
        "census": {"coverage": "full", "judged": 1, "in_scope": 1}
    })
}

#[cfg(feature = "private-test-util")]
fn job(id: u64, platform: &str, attempt: u64, conclusion: &str) -> Value {
    json!({
        "id": id, "run_id": 42, "run_attempt": attempt,
        "name": format!("cbh-collect:folo:{platform}"),
        "status": "completed", "conclusion": conclusion
    })
}

#[cfg(feature = "private-test-util")]
fn successful_jobs() -> Value {
    json!([
        job(1, "linux", 2, "success"),
        job(2, "windows", 2, "success")
    ])
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn workflow_matrix_is_offline_and_appends_shared_workflow_inputs() {
    let fixture = Fixture::new();
    let file = fixture.path("github-output");
    fs::write(&file, "earlier=value\n").unwrap();
    let output = fixture
        .command()
        .args([
            "--instance",
            "portable",
            "workflow-matrix",
            "--platforms",
            " windows,linux,windows ",
            "--github-output",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(file).unwrap(),
        concat!(
            "earlier=value\n",
            "matrix={\"platform\":[\"linux\",\"windows\"]}\n",
            "expected-platforms=linux,windows\n",
            "instance=portable\n",
            "collection-job-prefix=cbh-collect:portable\n"
        )
    );
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn invalid_workflow_matrix_cannot_append_partial_setup_outputs() {
    let fixture = Fixture::new();
    let file = fixture.path("github-output");
    fs::write(&file, "earlier=value\n").unwrap();
    let output = fixture
        .command()
        .args([
            "workflow-matrix",
            "--platforms",
            "linux,../escape",
            "--github-output",
        ])
        .arg(&file)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(file).unwrap(), "earlier=value\n");
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn collection_receipt_is_offline_and_preserves_actual_machine_key() {
    let fixture = Fixture::new();
    fs::write(fixture.path("machine-key.txt"), "0123456789ABCDEF\r\n").unwrap();
    let file = fixture.path("artifact").join("receipt.json");
    let output = fixture.receipt("windows", &head(), &file);
    assert!(output.status.success(), "{output:?}");
    let receipt: Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        receipt,
        json!({
            "version": 1, "repository": "folo-rs/folo", "instance": "folo",
            "run_id": 42, "run_attempt": 2, "head": head(), "platform": "windows",
            "machine_key": "0123456789abcdef"
        })
    );
    assert!(!fixture.receipt("windows", &head(), &file).status.success());
    assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn collection_rejects_bad_keys_dirty_heads_and_unsafe_platforms_without_output() {
    let fixture = Fixture::new();
    let output = fixture.path("receipt.json");
    fs::write(fixture.path("machine-key.txt"), "not a real machine key").unwrap();
    assert!(!fixture.receipt("linux", &head(), &output).status.success());
    fs::write(fixture.path("machine-key.txt"), "0123456789abcdef").unwrap();
    assert!(
        !fixture
            .receipt("../outside", &head(), &output)
            .status
            .success()
    );
    assert!(
        !fixture
            .receipt("linux", &(head() + "-dirty"), &output)
            .status
            .success()
    );
    assert!(!output.exists());
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn receipt_output_rejects_parent_traversal_even_after_a_missing_component() {
    let fixture = Fixture::new();
    fs::write(fixture.path("machine-key.txt"), "0123456789abcdef").unwrap();
    let destination = fixture.path("missing").join("..").join("receipt.json");
    assert!(
        !fixture
            .receipt("linux", &head(), &destination)
            .status
            .success()
    );
    assert!(!fixture.path("receipt.json").exists());
    assert!(!fixture.path("missing").exists());
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn inspected_outputs_append_offline_and_retain_existing_workflow_values() {
    let fixture = Fixture::new();
    fs::write(
        fixture.path("report.json"),
        serde_json::to_vec(&report("history")).unwrap(),
    )
    .unwrap();
    fs::write(fixture.path("github-output"), "earlier=value").unwrap();
    let output = fixture.inspect(&head(), "windows,linux");
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        fs::read_to_string(fixture.path("github-output")).unwrap(),
        "earlier=value\noutcome=clean\nnotable=false\ncan-clear=true\npublication-state=clean\n"
    );
    fs::write(
        fixture.path("report.json"),
        serde_json::to_vec(&report("branch")).unwrap(),
    )
    .unwrap();
    assert!(fixture.inspect(&head(), "linux").status.success());
    assert!(
        fs::read_to_string(fixture.path("github-output"))
            .unwrap()
            .ends_with(
                "outcome=clean\nnotable=false\ncan-clear=false\npublication-state=no-data\n"
            )
    );
}

#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn malformed_or_mismatched_reports_never_append_success_outputs() {
    let fixture = Fixture::new();
    fs::write(fixture.path("report.json"), "{}").unwrap();
    assert!(!fixture.inspect(&head(), "linux").status.success());
    assert!(!fixture.path("github-output").exists());
    fs::write(
        fixture.path("report.json"),
        serde_json::to_vec(&report("history")).unwrap(),
    )
    .unwrap();
    assert!(!fixture.inspect(&"b".repeat(40), "linux").status.success());
    assert!(!fixture.inspect(&head(), "unknown").status.success());
    assert!(!fixture.path("github-output").exists());
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn preparation_materializes_only_latest_successful_legs_machine_keys() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "fedcba9876543210");
    fs::write(fixture.path("github-output"), "earlier=value\n").unwrap();
    fixture
        .prepare(&json!([
            job(3, "windows", 3, "failure"),
            job(1, "linux", 2, "success"),
            job(2, "windows", 2, "success")
        ]))
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.path("keys").join("linux").join("machine-key.txt")).unwrap(),
        "0123456789abcdef\n"
    );
    assert!(!fixture.path("keys").join("windows").exists());
    assert_eq!(
        fs::read_to_string(fixture.path("github-output")).unwrap(),
        "earlier=value\ncompleted-platforms=linux\nmachine-keys=0123456789abcdef\ncomplete=false\n"
    );
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn receipt_only_preparation_sorts_platforms_and_machine_keys() {
    let fixture = Fixture::new();
    fixture.artifact("windows", "0123456789abcdef");
    fixture.artifact("linux", "fedcba9876543210");
    fixture.prepare(&successful_jobs()).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.path("github-output")).unwrap(),
        "completed-platforms=linux,windows\nmachine-keys=0123456789abcdef,fedcba9876543210\ncomplete=true\n"
    );
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn preparation_rejects_non_receipt_artifact_contents_before_writing_outputs() {
    for entry in ["results", "unexpected.json"] {
        let fixture = Fixture::new();
        fixture.artifact("linux", "0123456789abcdef");
        let windows = fixture.artifact("windows", "0123456789abcdef");
        let unexpected = windows.join(entry);
        if entry == "results" {
            fs::create_dir_all(&unexpected).unwrap();
        } else {
            fs::write(&unexpected, "unrelated").unwrap();
        }
        fixture.prepare(&successful_jobs()).unwrap_err();
        assert!(unexpected.exists());
        assert!(!fixture.path("keys").exists());
        assert!(!fixture.path("github-output").exists());
    }
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn preparation_requires_regular_receipts_in_artifact_directories() {
    for invalid in ["missing-receipt", "receipt-directory", "artifact-file"] {
        let fixture = Fixture::new();
        fixture.artifact("linux", "0123456789abcdef");
        let windows = fixture.artifact("windows", "0123456789abcdef");
        fs::remove_file(windows.join("receipt.json")).unwrap();
        match invalid {
            "receipt-directory" => fs::create_dir_all(windows.join("receipt.json")).unwrap(),
            "artifact-file" => {
                fs::remove_dir(&windows).unwrap();
                fs::write(&windows, "not a directory").unwrap();
            }
            _ => {}
        }
        fixture.prepare(&successful_jobs()).unwrap_err();
        assert!(!fixture.path("keys").exists());
        assert!(!fixture.path("github-output").exists());
    }
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn successful_retry_requires_a_new_receipt_before_writing_outputs() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "0123456789abcdef");
    fixture
        .prepare(&json!([
            job(1, "linux", 2, "success"),
            job(2, "windows", 2, "success"),
            job(3, "windows", 3, "success")
        ]))
        .unwrap_err();
    assert!(!fixture.path("keys").exists());
    assert!(!fixture.path("github-output").exists());
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn platforms_with_a_shared_machine_key_remain_independent_without_a_fake_report() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "0123456789abcdef");
    fixture.prepare(&successful_jobs()).unwrap();
    for platform in ["linux", "windows"] {
        assert_eq!(
            fs::read_to_string(fixture.path("keys").join(platform).join("machine-key.txt"))
                .unwrap(),
            "0123456789abcdef\n"
        );
    }
    assert!(!fixture.path("report.json").exists());
    assert_eq!(
        fs::read_to_string(fixture.path("github-output")).unwrap(),
        "completed-platforms=linux,windows\nmachine-keys=0123456789abcdef\ncomplete=true\n"
    );
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn preparation_does_not_clean_or_adopt_occupied_destinations() {
    for directory in [true, false] {
        let fixture = Fixture::new();
        fixture.artifact("linux", "0123456789abcdef");
        fixture.artifact("windows", "0123456789abcdef");
        let keys = fixture.path("keys");
        let sentinel = if directory {
            fs::create_dir_all(&keys).unwrap();
            keys.join("unrelated")
        } else {
            keys
        };
        fs::write(&sentinel, "keep").unwrap();
        fixture.prepare(&successful_jobs()).unwrap_err();
        assert_eq!(fs::read(sentinel).unwrap(), b"keep");
        assert!(!fixture.path("github-output").exists());
    }
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native destination planning and filesystem state.")]
fn rejected_destination_relationships_do_not_create_directories() {
    let keys = Path::new("keys");
    let receipts = Path::new("receipts");
    let output = Path::new("github-output");
    for (keys, output) in [
        (receipts.join("new-keys"), output.to_path_buf()),
        (keys.to_path_buf(), keys.to_path_buf()),
        (keys.to_path_buf(), keys.join("output")),
        (keys.to_path_buf(), receipts.join("output")),
        (keys.to_path_buf(), Path::new("missing").join("output")),
        (keys.join("..").join("outside"), output.to_path_buf()),
    ] {
        let fixture = Fixture::new();
        fixture.artifact("linux", "0123456789abcdef");
        fixture.artifact("windows", "0123456789abcdef");
        let keys = fixture.0.join(keys);
        let output = fixture.0.join(output);
        let before = fs::read_dir(&fixture.0).unwrap().count();
        fixture
            .prepare_to(&successful_jobs(), &keys, &output)
            .unwrap_err();
        assert!(!keys.exists());
        assert!(!output.exists());
        assert_eq!(fs::read_dir(&fixture.0).unwrap().count(), before);
        assert_eq!(fs::read_dir(fixture.path("receipts")).unwrap().count(), 2);
    }
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native destination planning and filesystem state.")]
fn invalid_output_file_does_not_create_machine_key_directories() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "0123456789abcdef");
    fs::create_dir_all(fixture.path("github-output")).unwrap();
    fixture.prepare(&successful_jobs()).unwrap_err();
    assert!(!fixture.path("keys").exists());
    assert!(
        fs::read_dir(fixture.path("github-output"))
            .unwrap()
            .next()
            .is_none()
    );
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native destination planning and materialization.")]
fn disjoint_missing_destination_ancestors_are_materialized_after_validation() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "0123456789abcdef");
    let keys = fixture.path("new-parent").join("keys");
    let output = fixture.path("github-output");
    fixture
        .prepare_to(&successful_jobs(), &keys, &output)
        .unwrap();
    assert!(keys.join("linux").join("machine-key.txt").is_file());
    assert!(output.is_file());
}

#[cfg(feature = "private-test-util")]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn malformed_artifacts_do_not_narrow_successful_platform_coverage() {
    let fixture = Fixture::new();
    fixture.artifact("linux", "0123456789abcdef");
    let windows = fixture.artifact("windows", "0123456789abcdef");
    fs::write(windows.join("receipt.json"), "{}").unwrap();
    fixture.prepare(&successful_jobs()).unwrap_err();
    assert!(!fixture.path("keys").exists());
    assert!(!fixture.path("github-output").exists());
}

#[cfg(all(unix, feature = "private-test-util"))]
#[test]
#[cfg_attr(miri, ignore = "Native collection artifact adapter coverage.")]
fn preparation_rejects_linked_receipts() {
    let fixture = Fixture::new();
    let linux = fixture.artifact("linux", "0123456789abcdef");
    fixture.artifact("windows", "0123456789abcdef");
    let receipt = linux.join("receipt.json");
    let outside = fixture.path("outside");
    fs::rename(&receipt, &outside).unwrap();
    symlink(&outside, &receipt).unwrap();
    fixture.prepare(&successful_jobs()).unwrap_err();
    assert!(!fixture.path("keys").exists());
    assert!(!fixture.path("github-output").exists());
}

#[cfg(unix)]
#[test]
#[cfg_attr(miri, ignore = "Native filesystem and child-process adapter coverage.")]
fn collection_rejects_linked_input_and_output_ancestors() {
    let fixture = Fixture::new();
    fs::write(fixture.path("real-key"), "0123456789abcdef").unwrap();
    symlink(fixture.path("real-key"), fixture.path("machine-key.txt")).unwrap();
    assert!(
        !fixture
            .receipt("linux", &head(), &fixture.path("receipt.json"))
            .status
            .success()
    );
    fs::remove_file(fixture.path("machine-key.txt")).unwrap();
    fs::rename(fixture.path("real-key"), fixture.path("machine-key.txt")).unwrap();
    fs::create_dir_all(fixture.path("outside")).unwrap();
    symlink(fixture.path("outside"), fixture.path("artifact")).unwrap();
    assert!(
        !fixture
            .receipt(
                "linux",
                &head(),
                &fixture.path("artifact").join("receipt.json")
            )
            .status
            .success()
    );
    assert!(
        fs::read_dir(fixture.path("outside"))
            .unwrap()
            .next()
            .is_none()
    );
}
