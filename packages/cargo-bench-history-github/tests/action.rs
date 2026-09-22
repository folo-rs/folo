//! Native post-install action boundaries with an isolated Git checkout and executable fixture.
//! These tests invoke real filesystem/process adapters without benchmark, storage or GitHub writes.

#![allow(
    clippy::indexing_slicing,
    reason = "Tests mutate known JSON object fixtures; indexing identifies the intended field directly."
)]

use std::env::consts::EXE_SUFFIX;
use std::env::var_os;
use std::ffi::OsString;
use std::fs;
#[cfg(unix)]
use std::os::unix::ffi::OsStringExt as _;
#[cfg(windows)]
use std::os::windows::ffi::OsStringExt as _;
#[cfg(windows)]
use std::path::{Component, Prefix};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::{Value, json};
use tempfile::{TempDir, tempdir};

/// Owns one measured checkout and an external job-temporary root.
struct Fixture {
    root: TempDir,
    checkout: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempdir().unwrap();
        let checkout = root.path().join("measured");
        fs::create_dir_all(&checkout).unwrap();
        Self { root, checkout }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .current_dir(&self.checkout)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn initialize_git(&self) {
        self.git(&["init", "--quiet"]);
        fs::write(self.checkout.join("README"), "measured checkout\n").unwrap();
        self.git(&["add", "README"]);
        self.git(&[
            "-c",
            "user.name=Action Test",
            "-c",
            "user.email=action@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ]);
    }

    fn command(&self, input: &Value) -> Command {
        self.command_with_temp(input, self.root.path())
    }

    fn command_with_temp(&self, input: &Value, temp: &Path) -> Command {
        fs::write(self.path("inputs.json"), serde_json::to_vec(input).unwrap()).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-github"));
        command
            .current_dir(&self.checkout)
            .args(["action", "--inputs-file"])
            .arg(self.path("inputs.json"))
            .arg("--github-output")
            .arg(self.path("outputs"))
            .arg("--temp-dir")
            .arg(temp);
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "GITHUB_REPOSITORY",
            "GITHUB_EVENT_NAME",
            "GITHUB_EVENT_PATH",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
            "GITHUB_SHA",
            "GITHUB_SERVER_URL",
            "CBH_ACTION_FIXTURE_FAIL",
            "CBH_ACTION_FIXTURE_CHECK_FLAGS",
            "CBH_ACTION_FIXTURE_EXPECT_RUSTFLAGS",
            "CBH_ACTION_FIXTURE_EXPECT_CARGO_ENCODED_RUSTFLAGS",
        ] {
            command.env_remove(name);
        }
        command
    }

    fn build_tool(&self) -> PathBuf {
        let tool = self.path(&format!("main-tool{EXE_SUFFIX}"));
        let output = Command::new("rustc")
            .arg(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("tests")
                    .join("fixtures")
                    .join("action_tool.rs"),
            )
            .arg("--edition=2024")
            .arg("-Dwarnings")
            .arg("-o")
            .arg(&tool)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        tool
    }

    fn tool_command(&self, input: &Value, tool: &Path) -> Command {
        self.tool_command_with_temp(input, tool, self.root.path())
    }

    fn tool_command_with_temp(&self, input: &Value, tool: &Path, temp: &Path) -> Command {
        let mut command = self.command_with_temp(input, temp);
        command
            .arg("--tool")
            .arg(tool)
            .env("CBH_ACTION_FIXTURE_LOG", self.path("tool.log"))
            .env("CBH_ACTION_FIXTURE_CHECKOUT", &self.checkout);
        for (name, value) in [
            ("GITHUB_TOKEN", "fixture-github-token"),
            ("GH_TOKEN", "fixture-gh-token"),
        ] {
            command
                .env(name, value)
                .env(format!("CBH_ACTION_FIXTURE_EXPECT_{name}"), value);
        }
        command
    }

    fn outputs(&self) -> String {
        fs::read_to_string(self.path("outputs")).unwrap()
    }

    fn report_path(&self, outputs: &str, key: &str) -> PathBuf {
        let path = outputs
            .lines()
            .rev()
            .find_map(|line| line.strip_prefix(key))
            .unwrap();
        let path = Path::new(path);
        assert!(path.is_absolute());
        #[cfg(windows)]
        assert!(matches!(
            path.components().next(),
            Some(Component::Prefix(prefix))
                if matches!(prefix.kind(), Prefix::Disk(_) | Prefix::UNC(_, _))
        ));
        assert!(path.is_file());
        let canonical = fs::canonicalize(path).unwrap();
        assert_eq!(fs::read(path).unwrap(), fs::read(&canonical).unwrap());
        assert!(!canonical.starts_with(fs::canonicalize(&self.checkout).unwrap()));
        canonical
    }
}

fn success(output: &Output) {
    assert!(output.status.success(), "{output:?}");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Real Git, executable compilation, child processes and temporary files."
)]
fn native_core_commands_preserve_environment_checkout_streams_and_reports() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let tool = fixture.build_tool();
    let key_dir = fixture.path("keys").join("linux");
    fs::create_dir_all(&key_dir).unwrap();
    fs::write(key_dir.join("machine-key.txt"), "0123456789ABCDEF\n").unwrap();
    fs::write(fixture.path("outputs"), "earlier=value").unwrap();

    let collect = fixture
        .tool_command(
            &json!({
                "command":"collect", "working-directory":"measured",
                "exclude":"omit", "bench":"speed", "all-features":"false"
            }),
            &tool,
        )
        .current_dir(fixture.root.path())
        .output()
        .unwrap();
    success(&collect);
    assert!(
        String::from_utf8(collect.stdout)
            .unwrap()
            .contains("fixture benchmark stdout")
    );
    assert!(
        String::from_utf8(collect.stderr)
            .unwrap()
            .contains("fixture benchmark stderr")
    );
    assert_eq!(
        fixture.outputs(),
        "earlier=value\ninstance=measured\nmachine-key=0123456789abcdef\n"
    );

    success(
        &fixture
            .tool_command(
                &json!({
                    "command":"backfill", "from":"HEAD", "to":"HEAD", "ignore-errors":"true"
                }),
                &tool,
            )
            .env_remove("GITHUB_TOKEN")
            .env_remove("GH_TOKEN")
            .env_remove("CBH_ACTION_FIXTURE_EXPECT_GITHUB_TOKEN")
            .env_remove("CBH_ACTION_FIXTURE_EXPECT_GH_TOKEN")
            .output()
            .unwrap(),
    );

    let mut reports = Vec::new();
    for command in ["analyze-history", "analyze-pr"] {
        success(
            &fixture
                .tool_command(
                    &json!({
                        "command":command, "machine-keys":fixture.path("keys"),
                        "local-path":fixture.path("store"), "expected-platforms":"linux,windows",
                        "completed-platforms":"linux"
                    }),
                    &tool,
                )
                .output()
                .unwrap(),
        );
        let outputs = fixture.outputs();
        assert!(outputs.contains("outcome=findings\n"));
        assert!(outputs.contains("partial-platform-coverage=true\n"));
        assert!(outputs.contains("regressions=2\n"));
        for key in ["report-json=", "report-markdown=", "report-summary="] {
            let canonical = fixture.report_path(&outputs, key);
            if key == "report-json=" {
                reports.push(canonical);
            }
        }
    }
    assert_ne!(
        reports.first().unwrap().parent(),
        reports.last().unwrap().parent()
    );
    assert!(fixture.git(&["status", "--porcelain"]).is_empty());
    let log = fs::read_to_string(fixture.path("tool.log")).unwrap();
    assert!(log.contains("--workspace"));
    assert!(log.contains("--ignore-errors"));
    assert!(!log.contains("--skip-existing"));
    assert!(!log.contains("--config"));
    assert!(log.contains("--machine-key=0123456789abcdef"));

    let before = fixture.outputs();
    for failure in ["collect", "machine-key", "non-utf8"] {
        let failed = fixture
            .tool_command(&json!({"command":"collect"}), &tool)
            .env("CBH_ACTION_FIXTURE_FAIL", failure)
            .output()
            .unwrap();
        assert!(!failed.status.success());
        assert_eq!(fixture.outputs(), before);
    }
    let failed = fixture
        .tool_command(&json!({"command":"collect"}), &fixture.path("missing-tool"))
        .output()
        .unwrap();
    assert!(!failed.status.success());
    assert_eq!(fixture.outputs(), before);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Real executable compilation, child processes and files."
)]
fn native_compiler_flags_override_only_measurement_children() {
    let fixture = Fixture::new();
    let tool = fixture.build_tool();
    let inherited = (var_os("RUSTFLAGS"), var_os("CARGO_ENCODED_RUSTFLAGS"));
    for (additional, ambient, encoded, expected) in [
        (
            None,
            Some("  --cfg=ordinary\t"),
            Some("--cfg=encoded\u{1f}-C\u{1f}link-arg=path with spaces"),
            Some("--cfg=encoded\u{1f}-C\u{1f}link-arg=path with spaces"),
        ),
        (Some(""), Some("  --cfg=ordinary\t"), None, None),
        (Some("--cfg=extra"), None, None, Some("--cfg=extra")),
        (
            Some(" -C llvm-args=-align-all-functions=6\t--cfg=extra "),
            Some("-Cllvm-args=-align-all-functions=3\t--cfg=ordinary"),
            None,
            Some(
                "-Cllvm-args=-align-all-functions=3\u{1f}--cfg=ordinary\u{1f}-C\u{1f}llvm-args=-align-all-functions=6\u{1f}--cfg=extra",
            ),
        ),
        (
            Some("-Cllvm-args=-align-all-functions=6"),
            Some("--cfg=ignored"),
            Some("-C\u{1f}link-arg=path with spaces"),
            Some("-C\u{1f}link-arg=path with spaces\u{1f}-Cllvm-args=-align-all-functions=6"),
        ),
        (
            Some("--cfg=extra"),
            Some("--cfg=ignored"),
            Some(""),
            Some("--cfg=extra"),
        ),
    ] {
        for action in ["collect", "backfill"] {
            let mut input = json!({"command":action});
            if let Some(additional) = additional {
                input["rustflags"] = json!(additional);
            }
            if action == "backfill" {
                input["from"] = json!("older");
                input["to"] = json!("newer");
            }
            let mut command = fixture.tool_command(&input, &tool);
            command.env("CBH_ACTION_FIXTURE_CHECK_FLAGS", "true");
            for (name, value) in [
                ("RUSTFLAGS", ambient),
                ("CARGO_ENCODED_RUSTFLAGS", encoded),
                ("CBH_ACTION_FIXTURE_EXPECT_RUSTFLAGS", ambient),
                (
                    "CBH_ACTION_FIXTURE_EXPECT_CARGO_ENCODED_RUSTFLAGS",
                    expected,
                ),
            ] {
                if let Some(value) = value {
                    command.env(name, value);
                } else {
                    command.env_remove(name);
                }
            }
            success(&command.output().unwrap());
        }
    }
    assert_eq!(
        (var_os("RUSTFLAGS"), var_os("CARGO_ENCODED_RUSTFLAGS")),
        inherited
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Real Git, subprocess and filesystem artifact-path boundary."
)]
fn native_artifact_paths_require_literal_glob_handoff() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let tool = fixture.build_tool();
    let keys = fixture.path("keys");
    fs::create_dir_all(&keys).unwrap();
    fs::write(keys.join("machine-key.txt"), "0123456789abcdef").unwrap();
    let input = json!({
        "command":"analyze-history", "machine-keys":keys,
        "local-path":fixture.path("store"),
        "expected-platforms":"canary", "completed-platforms":"canary"
    });
    let mut cases = vec![
        ("class[ab]", false),
        ("literal{a,b}", true),
        ("literal]suffix", true),
    ];
    if !cfg!(windows) {
        cases.extend([
            ("wild*card", false),
            ("wild?card", false),
            (r"literal\name", false),
        ]);
    }
    for (name, accepted) in cases {
        let temp = fixture.path(name);
        fs::create_dir_all(&temp).unwrap();
        fs::write(fixture.path("outputs"), "prior=untouched\n").unwrap();
        let result = fixture
            .tool_command_with_temp(&input, &tool, &temp)
            .output()
            .unwrap();
        if accepted {
            success(&result);
            let outputs = fixture.outputs();
            for key in ["report-markdown=", "report-json=", "report-summary="] {
                assert!(
                    fixture
                        .report_path(&outputs, key)
                        .starts_with(fs::canonicalize(&temp).unwrap())
                );
            }
        } else {
            assert!(!result.status.success());
            assert_eq!(fixture.outputs(), "prior=untouched\n");
            // Core analysis really completed; unsupported handoff paths must not turn that
            // into success outputs or an escaped spelling that names a different file.
            let directory = fs::read_dir(&temp).unwrap().next().unwrap().unwrap().path();
            let report: Value =
                serde_json::from_slice(&fs::read(directory.join("report.json")).unwrap()).unwrap();
            assert_eq!(report.get("outcome").unwrap(), "findings");
        }
    }
    assert!(fixture.git(&["status", "--porcelain"]).is_empty());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Real filesystem, core configuration loading and child process boundary."
)]
fn native_working_directory_config_and_fork_gate_need_no_tool_or_credential() {
    let fixture = Fixture::new();
    fs::write(
        fixture.checkout.join("settings.toml"),
        "[project]\nid='Mixed Project!'\n",
    )
    .unwrap();
    let event = fixture.path("event.json");
    fs::write(
        &event,
        serde_json::to_vec(&json!({
            "number":7,
            "pull_request":{
                "head":{"sha":"a".repeat(40), "repo":{"full_name":"fork/repo"}},
                "base":{"sha":"b".repeat(40), "repo":{"full_name":"owner/repo"}}
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let mut command = fixture.command(&json!({
        "command":"collect", "working-directory":"measured", "config":"settings.toml"
    }));
    command
        .current_dir(fixture.root.path())
        .env("GITHUB_EVENT_NAME", "pull_request_target")
        .env("GITHUB_EVENT_PATH", event)
        .arg("--tool")
        .arg(fixture.path("does-not-exist"));
    success(&command.output().unwrap());
    assert_eq!(
        fixture.outputs(),
        "instance=mixed_project_\nskipped=true\nskip-reason=fork-pull-request\n"
    );
}

#[test]
#[cfg_attr(miri, ignore = "Real filesystem, Git and native failure boundaries.")]
fn native_analysis_rejects_checkout_temp_cache_shallow_history_and_invalid_key_files() {
    let fixture = Fixture::new();
    fixture.initialize_git();
    let key_dir = fixture.path("keys");
    fs::create_dir_all(&key_dir).unwrap();
    fs::write(key_dir.join("machine-key.txt"), "0123456789abcdef").unwrap();
    let input = json!({"command":"analyze-history", "machine-keys":key_dir,
        "expected-platforms":"linux", "completed-platforms":"linux"});
    let tool = fixture.build_tool();
    let mut temp_input = input.clone();
    fs::create_dir_all(fixture.checkout.join("nested")).unwrap();
    temp_input["working-directory"] = json!("nested");
    let mut command = fixture.command_with_temp(&temp_input, &fixture.checkout);
    command
        .arg("--tool")
        .arg(&tool)
        .env("CBH_ACTION_FIXTURE_LOG", fixture.path("tool.log"));
    assert!(!command.output().unwrap().status.success());
    assert!(!fixture.path("outputs").exists());
    let mut cache = input.clone();
    cache["cache"] = json!("new-cache");
    assert!(
        !fixture
            .tool_command(&cache, &tool)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!fixture.checkout.join("new-cache").exists());
    fs::write(key_dir.join("unexpected.txt"), "0123456789abcdef").unwrap();
    assert!(
        !fixture
            .tool_command(&input, &tool)
            .output()
            .unwrap()
            .status
            .success()
    );
    fs::remove_file(key_dir.join("unexpected.txt")).unwrap();
    fs::write(
        fixture.checkout.join(".git").join("shallow"),
        fixture.git(&["rev-parse", "HEAD"]),
    )
    .unwrap();
    assert!(
        !fixture
            .tool_command(&input, &tool)
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!fixture.path("outputs").exists());
    assert!(!fixture.path("tool.log").exists());
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native argument validation and output-file preservation."
)]
fn native_invalid_inputs_and_publication_metadata_never_emit_success() {
    let fixture = Fixture::new();
    fs::write(fixture.path("outputs"), "prior=untouched\n").unwrap();
    for input in [
        json!({"command":"collect", "since":"30d"}),
        json!({"command":"collect", "best-of":1}),
        json!({"command":"alert"}),
        json!({"command":"publish-issue-failed", "conclusion":"success"}),
    ] {
        assert!(!fixture.command(&input).output().unwrap().status.success());
        assert_eq!(fixture.outputs(), "prior=untouched\n");
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native environment decoding, missing files and process startup failures."
)]
fn native_missing_inputs_paths_environment_and_credentials_are_explicit_failures() {
    let fixture = Fixture::new();
    let mut command = fixture.command(&json!({"command":"collect"}));
    fs::remove_file(fixture.path("inputs.json")).unwrap();
    assert!(!command.output().unwrap().status.success());

    assert!(
        !fixture
            .command(&json!({"command":"collect", "working-directory":"missing"}))
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !fixture
            .command(&json!({"command":"collect", "config":"missing.toml"}))
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(
        !fixture
            .command(&json!({"command":"analyze-history", "machine-keys":"keys",
        "expected-platforms":"linux", "completed-platforms":"linux"}))
            .env("PATH", "")
            .output()
            .unwrap()
            .status
            .success()
    );

    #[cfg(windows)]
    let invalid = OsString::from_wide(&[0xd800]);
    #[cfg(unix)]
    let invalid = OsString::from_vec(vec![255]);
    assert!(
        !fixture
            .command(&json!({"command":"collect"}))
            .env("GITHUB_SHA", invalid)
            .output()
            .unwrap()
            .status
            .success()
    );

    assert!(
        !fixture
            .command(&json!({"command":"alert", "run-id":"42",
        "run-url":"https://github.com/owner/repo/actions/runs/42"}))
            .env("GITHUB_REPOSITORY", "owner/repo")
            .output()
            .unwrap()
            .status
            .success()
    );
    assert!(!fixture.path("outputs").exists());
}
