//! Scaffolding shared by the integration modules.
//!
//! Provides fixture workspaces that are already seeded, and thin wrappers that
//! drive one run and reduce its outcome to what a test asserts on.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use cargo_release_plan::{CheckFormat, RunInput, RunOutcome, run};

use crate::fixture::{Fixture, write_package};

pub(crate) fn seeded_package() -> Fixture {
    Fixture::from_template(&SEEDED_PACKAGE)
}

/// A known ordinary-file workspace, committed once and only ever copied.
///
/// The real Git seed preserves the index and tree modes. Each caller receives
/// independent files, refs, objects and configuration before changing its state.
static SEEDED_PACKAGE: LazyLock<Fixture> = LazyLock::new(|| {
    let fixture = Fixture::new("");
    write_package(&fixture, "demo", "0.1.0", "");
    fixture.commit("seed");
    fixture
});

/// Workspace whose declared member contains a second package reached by path.
///
/// `packages/*` matches `packages/outer` only, so `inner` is a member solely
/// through the path dependency, and it sits inside the outer package directory.
pub(crate) fn nested_workspace() -> Fixture {
    let fixture = Fixture::new("");
    write_package(
        &fixture,
        "outer",
        "0.1.0",
        "\n[dependencies]\ninner = { path = \"inner\", version = \"0.1.0\" }",
    );
    fixture.write(
        "packages/outer/inner/Cargo.toml",
        "[package]\nname = \"inner\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    fixture.write("packages/outer/inner/src/lib.rs", "pub fn g() {}\n");
    fixture.commit("seed");
    fixture
}

pub(crate) fn check(fixture: &Fixture, release_history: &str) -> (bool, String) {
    check_workspace(release_history, fixture.manifest())
}

pub(crate) fn check_result(
    fixture: &Fixture,
    release_history: &str,
) -> Result<(bool, String), String> {
    match run(&RunInput::Check {
        merge_target: None,
        release_history: Some(release_history.to_string()),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        config: None,
        verbose: false,
    }) {
        Ok(RunOutcome::Check {
            passed, message, ..
        }) => Ok((passed, message)),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => Err(format!("{error}")),
    }
}

pub(crate) fn check_verbose(fixture: &Fixture, release_history: &str) -> (bool, String) {
    match run(&RunInput::Check {
        merge_target: None,
        release_history: Some(release_history.to_string()),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        config: None,
        verbose: true,
    }) {
        Ok(RunOutcome::Check {
            passed, message, ..
        }) => (passed, message),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => panic!("{error}"),
    }
}

pub(crate) fn check_workspace(release_history: &str, manifest_path: PathBuf) -> (bool, String) {
    match run(&RunInput::Check {
        merge_target: None,
        release_history: Some(release_history.to_string()),
        manifest_path,
        format: CheckFormat::Text,
        verify_packaging: false,
        config: None,
        verbose: false,
    }) {
        Ok(RunOutcome::Check {
            passed, message, ..
        }) => (passed, message),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => panic!("{error}"),
    }
}

/// Runs `check` with no baseline, leaving the tool to discover one.
pub(crate) fn check_discovering_base(fixture: &Fixture) -> Result<(bool, String), String> {
    match run(&RunInput::Check {
        merge_target: None,
        release_history: None,
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: false,
        config: None,
        verbose: true,
    }) {
        Ok(RunOutcome::Check {
            passed, message, ..
        }) => Ok((passed, message)),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => Err(format!("{error}")),
    }
}

pub(crate) fn check_verifying_packaging(
    fixture: &Fixture,
    release_history: &str,
) -> (bool, String) {
    match run(&RunInput::Check {
        merge_target: None,
        release_history: Some(release_history.to_string()),
        manifest_path: fixture.manifest(),
        format: CheckFormat::Text,
        verify_packaging: true,
        config: None,
        verbose: false,
    }) {
        Ok(RunOutcome::Check {
            passed, warnings, ..
        }) => (passed, warnings),
        Ok(other) => panic!("expected check, got {other:?}"),
        Err(error) => panic!("{error}"),
    }
}

pub(crate) fn report_json(fixture: &Fixture, release_history: &str) -> String {
    let out_dir = fixture.path().join("out");
    run(&RunInput::Report {
        merge_target: None,
        out_dir: out_dir.clone(),
        release_history: Some(release_history.to_string()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    fs::read_to_string(out_dir.join("report.json")).unwrap()
}

pub(crate) fn resolved_plan(fixture: &Fixture, proposal: &Path) -> PathBuf {
    let prepared = prepare(fixture);
    let output = fixture.path().join("preview");
    run(&RunInput::Preview {
        plan: proposal.to_path_buf(),
        prepared,
        output: output.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    output.join("plan.json")
}

pub(crate) fn prepare(fixture: &Fixture) -> PathBuf {
    let prepared = fixture.path().join("prepared");
    run(&RunInput::Prepare {
        merge_target: None,
        output: prepared.clone(),
        release_history: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    prepared.join("prepared.json")
}
