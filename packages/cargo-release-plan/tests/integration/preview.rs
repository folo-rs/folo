//! Offline preparation, fixed-point release effects, and captured application.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use cargo_release_plan::{RunInput, RunOutcome, run};
use serde_json::{Value, json};

use crate::fixture::{Fixture, write_package};
use crate::harness::check;

fn prepare(fixture: &Fixture) -> PathBuf {
    let output = fixture.path().join("prepared");
    let RunOutcome::Prepare { message } = run(&RunInput::Prepare {
        output: output.clone(),
        base: Some("HEAD".to_owned()),
        manifest_path: fixture.manifest(),
        verbose: true,
    })
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        message,
        format!(
            "Refreshed the workspace lockfile offline and prepared release evidence in {}",
            output.display()
        )
    );
    output.join("prepared.json")
}

fn preview(fixture: &Fixture, prepared: PathBuf, increments: &Value) -> PathBuf {
    let proposed = fixture.path().join("proposal.json");
    fs::write(
        &proposed,
        serde_json::to_vec(&json!({"schema_version": 4, "increments": increments})).unwrap(),
    )
    .unwrap();
    let output = fixture.path().join("preview");
    let RunOutcome::Preview { message } = run(&RunInput::Preview {
        plan: proposed,
        prepared,
        output: output.clone(),
        manifest_path: fixture.manifest(),
        verbose: true,
    })
    .unwrap() else {
        panic!()
    };
    assert_eq!(
        message,
        format!(
            "Wrote complete resolved plan to {}",
            output.join("plan.json").display()
        )
    );
    output.join("plan.json")
}

fn versions(plan: &PathBuf) -> BTreeMap<String, String> {
    let plan: Value = serde_json::from_slice(&fs::read(plan).unwrap()).unwrap();
    plan.get("increments")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|increment| {
            (
                increment["name"].as_str().unwrap().to_owned(),
                increment["version"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn apply(fixture: &Fixture, plan: PathBuf) {
    run(&RunInput::Apply {
        plan,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: true,
    })
    .unwrap();
}

fn binary(fixture: &Fixture, name: &str, extra: &str) {
    write_package(fixture, name, "0.1.0", extra);
    fixture.write(&format!("packages/{name}/src/main.rs"), "fn main() {}\n");
}

#[test]
#[cfg_attr(miri, ignore = "spawns local Git and offline Cargo")]
fn workspace_bumps_expand_transitive_binary_closures_before_apply() {
    let fixture = Fixture::new("");
    write_package(&fixture, "core", "0.1.0", "");
    write_package(
        &fixture,
        "bridge",
        "0.1.0",
        "\n[dependencies]\ncore = { path = \"../core\", version = \"0.1.0\" }\n",
    );
    binary(
        &fixture,
        "tool",
        "\n[dependencies]\nbridge = { path = \"../bridge\", version = \"0.1.0\" }\n",
    );
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("initial binary closure");
    let old_lock = fixture.read("Cargo.lock");
    fixture.write("packages/core/src/new.rs", "pub fn changed() {}\n");
    fixture.git(&["add", "-N", "packages/core/src/new.rs"]);
    let index = fixture.git(&["ls-files", "--stage", "-z"]);
    let prepared = prepare(&fixture);
    let report: Value = serde_json::from_str(&fixture.read("prepared/report.json")).unwrap();
    let core = report
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package.get("name").unwrap() == "core")
        .unwrap();
    assert_eq!(core.get("status").unwrap(), "needs-increment");
    fixture.write("proposal.json", r#"{"schema_version":4,"increments":[]}"#);
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared: prepared.clone(),
        output: fixture.path().join("preview"),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("preview/plan.json").exists());
    let plan = preview(
        &fixture,
        prepared,
        &json!([{"name": "core", "level": "patch"}]),
    );
    let document: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let inputs = document.get("resolved").unwrap().get("inputs").unwrap();
    assert_eq!(
        fs::canonicalize(inputs.get("root").unwrap().as_str().unwrap()).unwrap(),
        fs::canonicalize(fixture.path()).unwrap()
    );
    assert_eq!(
        inputs.get("manifest").unwrap().as_str().unwrap(),
        "Cargo.toml"
    );
    assert_eq!(
        versions(&plan),
        BTreeMap::from([
            ("core".to_owned(), "0.1.1".to_owned()),
            ("bridge".to_owned(), "0.1.1".to_owned()),
            ("tool".to_owned(), "0.1.1".to_owned()),
        ])
    );
    assert_eq!(fixture.read("Cargo.lock"), old_lock);
    assert!(!fixture.path().join("preview/.prospective").exists());
    assert_eq!(
        fixture.read("preview/workspace/packages/core/src/new.rs"),
        "pub fn changed() {}\n"
    );
    let inspection = RunInput::InspectPlan {
        plan: plan.clone(),
        require_resolved: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    };
    let before = fixture.git(&["status", "--porcelain"]);
    let RunOutcome::ArtifactQuery { message } = run(&inspection).unwrap() else {
        panic!()
    };
    assert_eq!(
        serde_json::from_str::<Value>(&message).unwrap(),
        json!({
            "publication_targets": ["bridge", "core", "tool"],
            "evidence_manifest_path": document
                .get("resolved")
                .unwrap()
                .get("evidence_manifest_path")
                .unwrap()
        })
    );
    assert_eq!(fixture.git(&["status", "--porcelain"]), before);
    assert_eq!(fixture.read("Cargo.lock"), old_lock);
    assert!(fixture.read("packages/core/Cargo.toml").contains("0.1.0"));
    fixture.write(
        "preview/workspace/packages/core/src/new.rs",
        "pub fn stale_candidate() {}\n",
    );
    run(&inspection).unwrap_err();
    fixture.write(
        "preview/workspace/packages/core/src/new.rs",
        "pub fn changed() {}\n",
    );
    run(&RunInput::Apply {
        plan: plan.clone(),
        dry_run: true,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert_eq!(fixture.read("Cargo.lock"), old_lock);
    assert!(fixture.read("packages/core/Cargo.toml").contains("0.1.0"));
    fixture.write("packages/core/src/new.rs", "pub fn stale() {}\n");
    run(&inspection).unwrap_err();
    run(&RunInput::Apply {
        plan: plan.clone(),
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fixture.read("Cargo.lock"), old_lock);
    assert!(fixture.read("packages/core/Cargo.toml").contains("0.1.0"));
    fixture.write("packages/core/src/new.rs", "pub fn changed() {}\n");
    apply(&fixture, plan.clone());
    let (passed, diagnostics) = check(&fixture, "HEAD");
    assert!(passed, "{diagnostics}");
    let lock = fixture.read("Cargo.lock");
    assert_ne!(lock, old_lock);
    apply(&fixture, plan);
    assert_eq!(fixture.read("Cargo.lock"), lock);
    assert_eq!(fixture.git(&["ls-files", "--stage", "-z"]), index);
}

#[test]
#[cfg_attr(miri, ignore = "spawns local Git and offline Cargo")]
fn preparation_resolves_already_locked_registry_edges_before_grading() {
    let fixture = Fixture::new(
        "exclude = [\"vendor/parent\"]\n[patch.crates-io]\nparent = { path = \"vendor/parent\" }\n",
    );
    fixture.write(
        ".cargo/config.toml",
        "[source.crates-io]\nreplace-with = \"vendor\"\n[source.vendor]\ndirectory = \"vendor\"\n",
    );
    for version in ["1.0.0", "2.0.0"] {
        let path = format!("vendor/leaf-{version}");
        fixture.write(
            &format!("{path}/Cargo.toml"),
            &format!("[package]\nname = \"leaf\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        );
        fixture.write(&format!("{path}/src/lib.rs"), "");
        fixture.write(
            &format!("{path}/.cargo-checksum.json"),
            r#"{"files":{},"package":null}"#,
        );
    }
    fixture.write("vendor/parent/Cargo.toml",
        "[package]\nname = \"parent\"\nversion = \"1.0.0\"\nedition = \"2021\"\n[dependencies]\nleaf = \">=1, <3\"\n");
    fixture.write("vendor/parent/src/lib.rs", "");
    fixture.write(
        "vendor/parent/.cargo-checksum.json",
        r#"{"files":{},"package":null}"#,
    );
    binary(
        &fixture,
        "tool",
        "\n[dependencies]\nparent = \"1.0.0\"\nleaf = \"=1.0.0\"\n",
    );
    binary(&fixture, "other", "\n[dependencies]\nleaf = \"=2.0.0\"\n");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    let resolved = fixture.read("Cargo.lock");
    // Both registry identities remain present; only the parent's selected edge is old.
    let baseline = resolved.replace(
        "name = \"parent\"\nversion = \"1.0.0\"\ndependencies = [\n \"leaf 2.0.0\",",
        "name = \"parent\"\nversion = \"1.0.0\"\ndependencies = [\n \"leaf 1.0.0\",",
    );
    assert_ne!(baseline, resolved);
    fixture.write("Cargo.lock", &baseline);
    fixture.commit("older edge with both identities locked");
    // A locally supplied third-party manifest can require a different already-locked identity
    // without changing that parent's own version. Preparation must assess the resulting closure.
    fixture.write("vendor/parent/Cargo.toml",
        "[package]\nname = \"parent\"\nversion = \"1.0.0\"\nedition = \"2021\"\n[dependencies]\nleaf = \"2.0.0\"\n");
    let prepared = prepare(&fixture);
    let prepared_lock = fixture.read("Cargo.lock");
    assert_ne!(prepared_lock, baseline);
    let report: Value = serde_json::from_str(&fixture.read("prepared/report.json")).unwrap();
    let tool = report
        .get("packages")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["name"] == "tool")
        .unwrap();
    assert_eq!(tool["status"], "needs-increment");
    assert!(
        tool["changed"]
            .as_array()
            .unwrap()
            .iter()
            .all(|change| change["source"] == "lockfile")
    );
    let plan = preview(&fixture, prepared, &json!([]));
    assert_eq!(versions(&plan).get("tool"), Some(&"0.1.1".to_owned()));
    assert_eq!(fixture.read("Cargo.lock"), prepared_lock);
    // Reinstating the earlier binary installation closure is stale input, not permission
    // for apply to resolve it again or add another release target.
    fixture.write("Cargo.lock", &baseline);
    let manifest = fixture.read("packages/tool/Cargo.toml");
    run(&RunInput::Apply {
        plan: plan.clone(),
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fixture.read("packages/tool/Cargo.toml"), manifest);
    assert_eq!(fixture.read("Cargo.lock"), baseline);
    fixture.write("Cargo.lock", &prepared_lock);
    apply(&fixture, plan);
    assert!(check(&fixture, "HEAD").0);
}

#[test]
#[cfg_attr(miri, ignore = "spawns local Git and offline Cargo")]
fn plain_expansion_is_read_only_and_cannot_bypass_resolution() {
    let fixture = Fixture::new("");
    binary(&fixture, "tool", "");
    fixture.cargo(&["generate-lockfile", "--offline"]);
    fixture.commit("released binary");
    fixture.write(
        "proposal.json",
        r#"{"schema_version":4,"increments":[{"name":"tool","level":"patch"}]}"#,
    );
    let lock = fixture.read("Cargo.lock");
    let expanded = fixture.path().join("expanded.json");
    run(&RunInput::Expand {
        preserve_input: false,
        plan: fixture.path().join("proposal.json"),
        out: expanded.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    assert_eq!(fixture.read("Cargo.lock"), lock);
    run(&RunInput::Apply {
        plan: expanded,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert_eq!(fixture.read("Cargo.lock"), lock);
    assert!(fixture.read("packages/tool/Cargo.toml").contains("0.1.0"));
}

#[test]
#[cfg_attr(miri, ignore = "spawns local Git and offline Cargo")]
fn source_changes_after_preparation_invalidate_preview() {
    let fixture = Fixture::new("");
    write_package(&fixture, "library", "0.1.0", "");
    fixture.commit("released package");
    let prepared = prepare(&fixture);
    // Completion invalidation is independent of the old plan's contents.
    fixture.write("preview/plan.json", "previous completion");
    fixture.write("packages/library/src/lib.rs", "pub fn new_evidence() {}\n");
    fixture.write(
        "proposal.json",
        r#"{"schema_version":4,"increments":[{"name":"library","level":"patch"}]}"#,
    );
    run(&RunInput::Preview {
        plan: fixture.path().join("proposal.json"),
        prepared,
        output: fixture.path().join("preview"),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap_err();
    assert!(!fixture.path().join("preview/plan.json").exists());
}
