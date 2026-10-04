use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use crp_versioning::plan::SCHEMA_VERSION;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::compatibility::{anticipated_parent, checker_command, read_outcome};
use crate::fixture::{Fixture, write_package};

/// Owns a prepared child and its retained candidate over an unpublished parent API.
///
/// Every operation starts the real CLI; only the external checker is a local protocol fixture.
pub(crate) struct Assessment {
    pub(crate) fixture: Fixture,
    pub(crate) evidence: TempDir,
    pub(crate) parent: String,
    mode: &'static str,
    pub(crate) storage: PathBuf,
}

impl Assessment {
    pub(crate) fn new(mode: &'static str) -> Self {
        let (fixture, history, parent) = anticipated_parent();
        write_package(&fixture, "library", "1.1.1", "");
        fixture.write("packages/library/src/lib.rs", "pub fn existing() {}\n");
        fixture.write(
            ".cargo/config.toml",
            "[build]\ntarget-dir = 'configured-target'\n",
        );
        fixture.commit("child removes parent API");
        fixture.git(&["branch", "release-history", &history]);
        let evidence = TempDir::new().unwrap();
        let storage = match mode {
            "default" | "disabled" => fixture
                .path()
                .join("configured-target/cargo-release-plan/cache"),
            "environment" => evidence.path().join("build/cargo-release-plan/cache"),
            "relative" => evidence.path().join("relative-cache"),
            "absolute" => evidence.path().join("absolute-cache"),
            _ => panic!("unknown cache fixture mode"),
        };
        let assessment = Self {
            fixture,
            evidence,
            parent,
            mode,
            storage,
        };
        success(
            assessment
                .command("prepare")
                .args([
                    "--release-history",
                    "release-history",
                    "--merge-target",
                    "anticipated-parent",
                    "--output",
                ])
                .arg(assessment.path("prepared"))
                .output()
                .unwrap(),
        );
        let proposal = assessment.path("proposal.json");
        fs::write(
            &proposal,
            serde_json::to_vec(&json!({
                "schema_version": SCHEMA_VERSION,
                "increments": []
            }))
            .unwrap(),
        )
        .unwrap();
        success(
            assessment
                .command("preview")
                .arg("--prepared")
                .arg(assessment.path("prepared/prepared.json"))
                .arg("--plan")
                .arg(proposal)
                .arg("--output")
                .arg(assessment.path("preview"))
                .output()
                .unwrap(),
        );
        assessment
    }

    pub(crate) fn path(&self, path: &str) -> PathBuf {
        self.evidence.path().join(path)
    }

    pub(crate) fn command(&self, operation: &str) -> Command {
        let mut command = checker_command();
        command
            .current_dir(self.evidence.path())
            .env_remove("CARGO_TARGET_DIR")
            .args([operation, "--verbose", "--manifest-path"])
            .arg(self.fixture.manifest());
        match self.mode {
            "environment" => {
                command.env("CARGO_TARGET_DIR", self.path("build"));
            }
            "relative" => {
                command.args(["--cache", "relative-cache"]);
            }
            "absolute" => {
                command.arg("--cache").arg(&self.storage);
            }
            "disabled" => {
                command.arg("--no-cache");
            }
            "default" => {}
            _ => panic!("unknown cache fixture mode"),
        }
        command
    }

    pub(crate) fn check(&self, name: &str) -> Command {
        self.check_mode(name, "--plan", "preview/plan.json")
    }

    pub(crate) fn check_mode(&self, name: &str, option: &str, artifact: &str) -> Command {
        let mut command = self.command("check-compatibility");
        command
            .arg(option)
            .arg(self.path(artifact))
            .arg("--output")
            .arg(self.path(name))
            .env("CRP_FIXTURE_SCENARIO", "anticipated-parent")
            .env("CRP_EXPECTED_PARENT", &self.parent)
            .env("CRP_FIXTURE_CALLS", self.path(&format!("{name}.calls")))
            .env(
                "CRP_FIXTURE_SOURCE",
                self.fixture.path().join("packages/library/src/lib.rs"),
            );
        command
    }
}

pub(crate) fn success(output: Output) -> Output {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

pub(crate) fn reused(output: &Output) {
    let diagnostics = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostics.contains("reusing classification decisions from storage"),
        "{diagnostics}"
    );
    assert!(!diagnostics.contains("computed classification decisions"));
}

pub(crate) fn candidate(assessment: &Assessment) -> PathBuf {
    let plan: Value =
        serde_json::from_slice(&fs::read(assessment.path("preview/plan.json")).unwrap()).unwrap();
    Path::new(
        plan.pointer("/resolved/evidence_manifest_path")
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .parent()
    .unwrap()
    .to_path_buf()
}

pub(crate) fn same_evidence(expected: &Path, actual: &Path) {
    assert_eq!(
        fs::read(expected.join("report.json")).unwrap(),
        fs::read(actual.join("report.json")).unwrap()
    );
    let mut expected_outcome = read_outcome(expected);
    let mut actual_outcome = read_outcome(actual);
    for outcome in [&mut expected_outcome, &mut actual_outcome] {
        // Each invocation necessarily owns a different output report path.
        outcome.as_object_mut().unwrap().remove("report").unwrap();
    }
    assert_eq!(expected_outcome, actual_outcome);
    assert_eq!(
        fs::read(expected.join("semver-checks.log")).unwrap(),
        fs::read(actual.join("semver-checks.log")).unwrap()
    );
    for entry in fs::read_dir(expected.join("diffs")).unwrap() {
        let entry = entry.unwrap();
        assert_eq!(
            fs::read(entry.path()).unwrap(),
            fs::read(actual.join("diffs").join(entry.file_name())).unwrap()
        );
    }
}
