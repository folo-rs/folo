//! Real Git, Cargo metadata and detector queries for offline reusable-workflow preparation.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use serde_json::{Value, json};
use tempfile::TempDir;

/// Owns an isolated workspace plus event/config/output files outside the measured checkout.
struct Fixture {
    root: TempDir,
    checkout: PathBuf,
    base: String,
    head: String,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let checkout = root.path().join("measured");
        fs::create_dir_all(&checkout).unwrap();
        fs::write(
            checkout.join("Cargo.toml"),
            "[workspace]\nresolver='3'\nmembers=['library','benchmark','unrelated']\n",
        )
        .unwrap();
        for (name, bench, dependency) in [
            ("library", false, false),
            ("benchmark", true, true),
            ("unrelated", true, false),
        ] {
            let directory = checkout.join(name);
            fs::create_dir_all(directory.join("src")).unwrap();
            fs::write(directory.join("src").join("lib.rs"), "").unwrap();
            let mut manifest =
                format!("[package]\nname='{name}'\nversion='0.0.0'\nedition='2024'\n");
            if bench {
                manifest.push_str("[[bench]]\nname='probe'\nharness=false\n");
                fs::create_dir_all(directory.join("benches")).unwrap();
                fs::write(directory.join("benches").join("probe.rs"), "fn main() {}\n").unwrap();
            }
            if dependency {
                manifest.push_str("[dev-dependencies]\nlibrary={path='../library'}\n");
            }
            fs::write(directory.join("Cargo.toml"), manifest).unwrap();
        }
        fs::write(checkout.join("library").join("removed.rs"), "old source\n").unwrap();
        let output = Command::new("cargo")
            .current_dir(&checkout)
            .args(["generate-lockfile", "--offline"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let mut fixture = Self {
            root,
            checkout,
            base: String::new(),
            head: String::new(),
        };
        fixture.git(&["init", "--quiet"]);
        fixture.git(&["add", "."]);
        fixture.commit();
        fixture
            .git(&["rev-parse", "HEAD"])
            .trim()
            .clone_into(&mut fixture.base);
        fs::remove_file(fixture.checkout.join("library").join("removed.rs")).unwrap();
        fixture.git(&["add", "-u"]);
        fixture.commit();
        fixture
            .git(&["rev-parse", "HEAD"])
            .trim()
            .clone_into(&mut fixture.head);
        fixture
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

    fn commit(&self) {
        self.git(&[
            "-c",
            "user.name=Preparation Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ]);
    }

    fn command(&self, flow: &str, input: &Value) -> Command {
        let input_path = self.root.path().join("inputs.json");
        let event_path = self.root.path().join("event.json");
        fs::write(&input_path, serde_json::to_vec(input).unwrap()).unwrap();
        fs::write(
            &event_path,
            serde_json::to_vec(&json!({
                "number":7, "pull_request":{
                    "head":{"sha":self.head, "repo":{"full_name":"owner/repo"}},
                    "base":{"sha":self.base, "repo":{"full_name":"owner/repo"}}
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_cargo-bench-history-github"));
        command
            .current_dir(&self.checkout)
            .args(["prepare-workflow", "--flow", flow, "--inputs-file"])
            .arg(input_path)
            .arg("--github-output")
            .arg(self.root.path().join("outputs"));
        for name in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "GITHUB_EVENT_NAME",
            "GITHUB_EVENT_PATH",
            "GITHUB_REPOSITORY",
            "GITHUB_SHA",
            "GITHUB_RUN_ID",
            "GITHUB_RUN_ATTEMPT",
        ] {
            command.env_remove(name);
        }
        if flow == "pr" {
            command
                .env("GITHUB_EVENT_NAME", "pull_request")
                .env("GITHUB_EVENT_PATH", event_path)
                .env("GITHUB_SHA", "c".repeat(40));
        }
        command
    }

    fn prepare(&self, flow: &str, input: &Value) -> BTreeMap<String, String> {
        let output = self.command(flow, input).output().unwrap();
        assert!(output.status.success(), "{output:?}");
        let text = fs::read_to_string(self.root.path().join("outputs")).unwrap();
        text.lines()
            .map(|line| {
                let (key, value) = line.split_once('=').unwrap();
                (key.to_owned(), value.to_owned())
            })
            .collect()
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native Git, Cargo metadata, process and filesystem boundaries."
)]
fn native_history_workspace_and_pr_affected_deleted_file_scopes() {
    let fixture = Fixture::new();
    let config = fixture.root.path().join("authority.toml");
    fs::write(&config, "[project]\nid='Measured Project!'").unwrap();
    let mut input = json!({"platforms":"windows,linux", "config":config});
    let outputs = fixture.prepare("history", &input);
    assert_eq!(outputs.get("instance").unwrap(), "measured_project_");
    assert_eq!(outputs.get("packages").unwrap(), "benchmark,unrelated");
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
    assert_eq!(outputs.get("base").unwrap(), &fixture.head);
    assert_eq!(outputs.get("skip-all").unwrap(), "false");
    assert_eq!(
        outputs.get("collection-job-prefix").unwrap(),
        "cbh-collect:measured_project_"
    );
    _ = input
        .as_object_mut()
        .unwrap()
        .insert("exclude".to_owned(), json!("library"));
    let outputs = fixture.prepare("pr", &input);
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
    assert_eq!(outputs.get("base").unwrap(), &fixture.base);
    assert_eq!(outputs.get("packages").unwrap(), "benchmark");
    assert_eq!(outputs.get("skip-all").unwrap(), "false");
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native Git, Cargo metadata, process and filesystem boundaries."
)]
fn native_empty_scope_and_history_freeze_have_concrete_outputs() {
    let fixture = Fixture::new();
    let outputs = fixture.prepare(
        "history",
        &json!({
            "platforms":"linux", "exclude":"benchmark,unrelated",
        }),
    );
    assert_eq!(outputs.get("packages").unwrap(), "");
    assert_eq!(outputs.get("skip-all").unwrap(), "true");
    assert_eq!(outputs.get("head"), outputs.get("base"));
    assert_eq!(outputs.get("head").unwrap(), &fixture.head);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "Native checkout mismatch and append-only output boundary."
)]
fn native_bad_event_head_leaves_existing_outputs_untouched() {
    let fixture = Fixture::new();
    fs::write(fixture.root.path().join("outputs"), "previous=value\n").unwrap();
    let output = fixture
        .command("history", &json!({"platforms":"linux"}))
        .env("GITHUB_SHA", &fixture.base)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(fixture.root.path().join("outputs")).unwrap(),
        "previous=value\n"
    );
}
