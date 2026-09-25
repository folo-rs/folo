//! Subprocess coverage of the `cargo-release-plan` binary entry point.
//!
//! These cases cover only what is unique to the executable: which stream each
//! outcome is written to and which exit status it produces. Classification and
//! plan semantics are covered in-process by the other modules of this suite.

use std::fs;
use std::process::{Command, Output};

use tempfile::TempDir;

use crate::fixture::{Fixture, write_binary_package, write_package};

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn help_exits_success() {
    let output = release_plan(&["--help"], None);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn cargo_injected_subcommand_is_stripped() {
    let output = release_plan(&["release-plan", "--help"], None);
    assert!(output.status.success());
    assert!(stdout(&output).contains("Usage"));
}

#[cfg_attr(miri, ignore = "Spawns the compiled application in an empty directory")]
#[test]
fn version_reports_the_installed_application_without_a_workspace() {
    let directory = TempDir::new().unwrap();
    for args in [&["--version"][..], &["release-plan", "--version"][..]] {
        let output = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"))
            .args(args)
            .current_dir(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        assert!(stderr(&output).is_empty());
        assert_eq!(
            stdout(&output).trim(),
            format!("cargo-release-plan {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[cfg_attr(miri, ignore = "Spawns the application, Cargo and Git")]
#[test]
fn publication_configuration_is_explicit_and_does_not_replace_version_checks() {
    let fixture = seeded_package();
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/libs'\nrelease-branch = 'main'\ntargets = []",
    );
    fixture.commit("configure publication");
    let base = fixture.sha("HEAD");
    let args = [
        "check",
        "--base",
        &base,
        "--config",
        ".cargo/release_plan.toml",
    ];
    assert!(release_plan(&args, Some(&fixture)).status.success());
    fixture.write(".cargo/release_plan.toml", "not valid TOML");
    assert!(!release_plan(&args, Some(&fixture)).status.success());
    assert!(
        release_plan(&["check", "--base", &base], Some(&fixture))
            .status
            .success()
    );
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/libs'\nrelease-branch = 'main'\ntargets = []",
    );
    fixture.write("packages/demo/src/lib.rs", "pub fn new_operation() {}\n");
    assert!(!release_plan(&args, Some(&fixture)).status.success());
}

#[cfg_attr(miri, ignore = "Spawns Cargo, Git and the compiled application")]
#[test]
fn configured_binary_check_matches_cargos_default_feature_selection() {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "optional-core",
        "0.1.0",
        "[features]\nenhanced = []\n",
    );
    write_binary_package(
        &fixture,
        "tool",
        "0.1.0",
        r#"repository = "https://github.com/example/tools"
[[bin]]
name = "tool"
path = "src/main.rs"
required-features = ["optional-core"]
[dependencies]
optional-core = { path = "../optional-core", version = "0.1.0", optional = true }
[features]
default = ["optional-core/enhanced"]
[package.metadata.release-plan]
release-targets = ["x86_64-pc-windows-msvc"]
[package.metadata.binstall]
pkg-url = "{ repo }/releases/download/{ name }-v{ version }/{ name }-v{ version }-{ target }.zip"
bin-dir = "{ bin }{ binary-ext }"
pkg-fmt = "zip"
"#,
    );
    fixture.write(
        ".cargo/release_plan.toml",
        "schema-version = 1\nrepository = 'example/tools'\nrelease-branch = 'main'\n\
         targets = ['x86_64-unknown-linux-gnu', 'x86_64-pc-windows-msvc']",
    );
    fixture.write(".gitignore", "/target\n");
    let output = Command::new("cargo")
        .args(["build", "--bins", "--offline"])
        .current_dir(fixture.path())
        .env("CARGO_TARGET_DIR", fixture.path().join("target"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(fixture.path().join("target/debug").is_dir());
    fixture.commit("configured binary source");
    let base = fixture.sha("HEAD");
    let output = release_plan(
        &[
            "check",
            "--base",
            &base,
            "--config",
            ".cargo/release_plan.toml",
            "--verbose",
        ],
        Some(&fixture),
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stderr(&output).contains("x86_64-pc-windows-msvc"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary; Miri cannot emulate that.
#[test]
fn unknown_flag_exits_failure() {
    let output = release_plan(&["--definitely-not-a-flag"], None);
    assert!(!output.status.success());
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary and git; Miri cannot emulate that.
#[test]
fn passing_check_writes_stdout_and_exits_success() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    let output = release_plan(&["check", "--base", &base], Some(&fixture));

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary, git, and cargo; Miri cannot emulate that.
#[test]
fn passing_packaging_warnings_write_stderr_without_replacing_stdout() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    // The tool ignores untracked files while Cargo packages them, deliberately
    // producing a non-gating warning from the packaging cross-check.
    fixture.write("packages/demo/src/extra.rs", "pub fn g() {}\n");
    let output = release_plan(
        &["check", "--base", &base, "--verify-packaging"],
        Some(&fixture),
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("Every release and workspace-version check passed"),
        "{}",
        stdout(&output)
    );
    assert!(
        stderr(&output).contains("warning: packaging rule mismatch"),
        "{}",
        stderr(&output)
    );
    assert!(!stdout(&output).contains("warning:"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary and git; Miri cannot emulate that.
#[test]
fn failing_check_writes_stderr_and_exits_failure() {
    let fixture = seeded_package();
    fixture.write("packages/demo/src/lib.rs", "pub fn f() { let _ = 3; }\n");
    fixture.commit("content without a version increment");
    let base = fixture.sha("HEAD");
    let output = release_plan(&["check", "--base", &base], Some(&fixture));

    assert!(!output.status.success());
    assert!(stderr(&output).contains("needs-increment"));
    assert!(stdout(&output).is_empty());
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary and git; Miri cannot emulate that.
#[test]
fn operational_error_writes_stderr_and_exits_failure() {
    let fixture = seeded_package();
    // A base revision no repository resolves, so classification fails before it
    // can produce a verdict.
    let output = release_plan(&["check", "--base", "definitely-not-a-rev"], Some(&fixture));

    assert!(!output.status.success());
    assert!(stderr(&output).contains("Error:"));
    assert!(stdout(&output).is_empty());
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary and git; Miri cannot emulate that.
#[test]
fn report_writes_its_summary_to_stdout() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    let out_dir = fixture.path().join("out");
    let output = release_plan(
        &[
            "report",
            "--base",
            &base,
            "--out-dir",
            &out_dir.to_string_lossy(),
        ],
        Some(&fixture),
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("report.json"));
}

#[cfg_attr(miri, ignore)] // Spawns the compiled binary and git; Miri cannot emulate that.
#[test]
fn apply_writes_its_summary_to_stdout() {
    let fixture = seeded_package();
    let plan_path = fixture.path().join("plan.json");
    fs::write(
        &plan_path,
        r#"{ "schema_version": 4, "increments": [{ "name": "demo", "level": "patch" }] }"#,
    )
    .unwrap();
    let output = release_plan(
        &["apply", "--plan", &plan_path.to_string_lossy(), "--dry-run"],
        Some(&fixture),
    );

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(stdout(&output).contains("Dry run"));
}

#[test]
#[cfg_attr(miri, ignore = "drives the actual CLI, Git, and offline Cargo")]
fn resolved_workflow_dispatches_every_command_to_stdout() {
    let fixture = seeded_package();
    let base = fixture.sha("HEAD");
    let prepared = fixture.path().join("prepared");
    let preview = fixture.path().join("preview");
    let proposal = fixture.path().join("proposal.json");
    fs::write(
        &proposal,
        r#"{"schema_version":4,"increments":[{"name":"demo","level":"patch"}]}"#,
    )
    .unwrap();

    let output = release_plan(
        &[
            "prepare",
            "--base",
            &base,
            "--output",
            &prepared.to_string_lossy(),
        ],
        Some(&fixture),
    );
    assert!(output.status.success());
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());

    let output = release_plan(
        &[
            "preview",
            "--prepared",
            &prepared.join("prepared.json").to_string_lossy(),
            "--plan",
            &proposal.to_string_lossy(),
            "--output",
            &preview.to_string_lossy(),
        ],
        Some(&fixture),
    );
    assert!(output.status.success());
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());

    let plan = preview.join("plan.json");
    let candidate = preview.join("workspace/Cargo.toml");
    let output = release_plan(
        &[
            "verify-preview",
            "--plan",
            &plan.to_string_lossy(),
            "--manifest-path",
            &candidate.to_string_lossy(),
        ],
        Some(&fixture),
    );
    assert!(output.status.success());
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());

    let output = release_plan(
        &[
            "expand",
            "--plan",
            &plan.to_string_lossy(),
            "--out",
            &fixture.path().join("portable.json").to_string_lossy(),
        ],
        Some(&fixture),
    );
    assert!(output.status.success());
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());

    let output = release_plan(
        &["apply", "--plan", &plan.to_string_lossy()],
        Some(&fixture),
    );
    assert!(output.status.success());
    assert!(!stdout(&output).is_empty());
    assert!(stderr(&output).is_empty());
    assert!(fixture.read("packages/demo/Cargo.toml").contains("0.1.1"));
    assert!(
        release_plan(&["check", "--base", &base], Some(&fixture))
            .status
            .success()
    );

    let output = release_plan(
        &[
            "verify-preview",
            "--plan",
            &plan.to_string_lossy(),
            "--manifest-path",
            &fixture.manifest().to_string_lossy(),
        ],
        Some(&fixture),
    );
    assert!(!output.status.success());
    assert!(stdout(&output).is_empty());
    assert!(!stderr(&output).is_empty());
}

fn seeded_package() -> Fixture {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    fixture
}

fn release_plan(args: &[&str], fixture: Option<&Fixture>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-release-plan"));
    command.args(args);
    if let Some(fixture) = fixture {
        command.current_dir(fixture.path());
    }
    command.output().unwrap()
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}
