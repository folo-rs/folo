//! Real-source exposure fixtures exercise the complete release-planning boundary.

use std::collections::BTreeMap;
use std::fs;

use cargo_release_plan::{RunInput, run};
use serde_json::{Value, json};

use crate::fixture::{Fixture, write_package};
use crate::harness::{check, prepare};

/// Selects the public contract, independently of which package receives a breaking edit.
#[derive(Clone, Copy)]
enum Exposure {
    SharedCore,
    Synchronization,
    NestedAdapter,
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compiles downstream source and executes Git/Cargo planning"
)]
fn shared_origins_do_not_propagate_unrelated_supplier_breaks() {
    release(Exposure::SharedCore, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compiles downstream source and executes Git/Cargo planning"
)]
fn exposing_the_suppliers_own_type_requires_a_breaking_release() {
    release(Exposure::Synchronization, false);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compiles downstream source and executes Git/Cargo planning"
)]
fn a_breaking_core_reaches_independent_reexport_paths() {
    release(Exposure::SharedCore, true);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "compiles downstream source and executes Git/Cargo planning"
)]
fn nested_external_reexports_retain_origins_through_private_owners() {
    release(Exposure::NestedAdapter, true);
}

/// Verifies observations, semantic proposal, mechanical preview and applied readiness together.
fn release(exposure: Exposure, break_core: bool) {
    let fixture = workspace(exposure);
    fixture.cargo(&["check", "--offline", "-p", "consumer"]);
    fixture.commit("baseline");
    let core = fixture.read("packages/coordinate_core/src/lib.rs");
    let synchronization = fixture.read("packages/synchronization/src/lib.rs");
    if break_core {
        fixture.write(
            "packages/coordinate_core/src/lib.rs",
            &core.replace("pub struct LegacyCore;", ""),
        );
    } else {
        fixture.write(
            "packages/coordinate_core/src/lib.rs",
            &format!("//! Core documentation.\n{core}"),
        );
        fixture.write(
            "packages/synchronization/src/lib.rs",
            &synchronization.replace("pub struct LegacyTag;", ""),
        );
    }
    let clock = fixture.read("packages/clock/src/lib.rs");
    fixture.write(
        "packages/clock/src/lib.rs",
        &format!("//! Clock documentation.\n{clock}"),
    );
    let prepared = prepare(&fixture);
    let report: Value = serde_json::from_str(&fixture.read("prepared/report.json")).unwrap();
    assert_eq!(report.get("groups").unwrap(), &json!({}));
    let packages = report.get("packages").unwrap().as_array().unwrap();
    let clock = packages
        .iter()
        .find(|package| package.get("name").unwrap() == "clock")
        .unwrap();
    let origins = match exposure {
        Exposure::SharedCore => json!(["coordinate_core"]),
        Exposure::Synchronization | Exposure::NestedAdapter => {
            json!(["coordinate_core", "synchronization"])
        }
    };
    assert_eq!(clock.get("public_origins").unwrap(), &origins);
    assert!(
        clock
            .get("dependencies")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .all(|dependency| dependency.get("public").is_none())
    );

    let changes = if break_core {
        json!([
            {"name": "coordinate_core", "impact": "breaking"},
            {"name": "clock", "impact": "patch"}
        ])
    } else {
        json!([
            {"name": "coordinate_core", "impact": "patch"},
            {"name": "coordinate_facade", "impact": "patch"},
            {"name": "synchronization", "impact": "breaking"},
            {"name": "clock", "impact": "patch"}
        ])
    };
    fixture.write(
        "decisions.json",
        &json!({"schema_version": 2, "changes": changes}).to_string(),
    );
    let proposal = fixture.path().join("proposal.json");
    run(&RunInput::Propose {
        report: fixture.path().join("prepared/report.json"),
        decisions: fixture.path().join("decisions.json"),
        out: proposal.clone(),
        verbose: false,
    })
    .unwrap();
    let proposal_document: Value = serde_json::from_slice(&fs::read(&proposal).unwrap()).unwrap();
    let clock_breaks = break_core || matches!(exposure, Exposure::Synchronization);
    let clock_increment = proposal_document
        .get("increments")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .find(|increment| increment.get("name").unwrap() == "clock")
        .unwrap();
    assert_eq!(
        clock_increment.get("bump").unwrap(),
        if clock_breaks { "major" } else { "patch" }
    );
    let output = fixture.path().join("preview");
    run(&RunInput::Preview {
        plan: proposal,
        prepared,
        output: output.clone(),
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let plan = output.join("plan.json");
    let document: Value = serde_json::from_slice(&fs::read(&plan).unwrap()).unwrap();
    let versions = document
        .get("increments")
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|increment| {
            (
                increment.get("name").unwrap().as_str().unwrap(),
                increment.get("version").unwrap().as_str().unwrap(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        versions,
        BTreeMap::from([
            ("clock", if clock_breaks { "2.0.0" } else { "1.0.1" }),
            (
                "coordinate_core",
                if break_core { "2.0.0" } else { "1.0.1" }
            ),
            (
                "coordinate_facade",
                if break_core { "2.0.0" } else { "1.0.1" }
            ),
            ("synchronization", "2.0.0"),
        ])
    );
    run(&RunInput::Apply {
        plan,
        dry_run: false,
        manifest_path: fixture.manifest(),
        verbose: false,
    })
    .unwrap();
    let (passed, diagnostics) = check(&fixture, "HEAD");
    assert!(passed, "{diagnostics}");
    fixture.cargo(&["check", "--offline", "--locked", "-p", "consumer"]);
}

/// Compatible requirements keep grouping from imposing or hiding the disputed increment.
fn workspace(exposure: Exposure) -> Fixture {
    let fixture = Fixture::new("");
    fixture.write(".gitignore", "/target/\n");
    write_package(&fixture, "coordinate_core", "1.0.0", "");
    fixture.write(
        "packages/coordinate_core/src/lib.rs",
        "
pub struct Thread;
pub struct LegacyCore;
pub trait ThreadAware {
    fn relocate(&mut self, destination: &Thread);
}
",
    );
    let core_dependency = "\n[dependencies]\ncoordinate_core = { path = \"../coordinate_core\", version = \"1.0.0\" }\n";
    let core_allowlist = "\n[package.metadata.cargo_check_external_types]\nallowed_external_types = [\"coordinate_core::Thread\", \"coordinate_core::ThreadAware\"]\n";
    write_package(
        &fixture,
        "coordinate_facade",
        "1.0.0",
        &format!("{core_allowlist}{core_dependency}"),
    );
    fixture.write(
        "packages/coordinate_facade/src/lib.rs",
        "pub use coordinate_core::{Thread, ThreadAware};\n",
    );
    let private = if matches!(exposure, Exposure::NestedAdapter) {
        "\n[package.metadata.release-plan]\nprivate-api = true\n"
    } else {
        ""
    };
    write_package(
        &fixture,
        "synchronization",
        "1.0.0",
        &format!("{private}{core_allowlist}{core_dependency}"),
    );
    fixture.write(
        "packages/synchronization/src/lib.rs",
        "
use coordinate_core::{Thread, ThreadAware};
pub struct LegacyTag;
pub struct Mutex<T>(T);
impl<T> Mutex<T> {
    pub fn new(value: T) -> Self { Self(value) }
}
impl<T> ThreadAware for Mutex<T> {
    fn relocate(&mut self, _: &Thread) { let _ = &self.0; }
}
pub struct Adapter;
impl Adapter {
    pub fn accept(&self, _: &Thread) {}
}
",
    );
    let clock_allowlist = match exposure {
        Exposure::SharedCore => "\"coordinate_core::Thread\", \"coordinate_core::ThreadAware\"",
        Exposure::Synchronization => {
            "\"coordinate_core::Thread\", \"coordinate_core::ThreadAware\", \"synchronization::Mutex\""
        }
        Exposure::NestedAdapter => "\"synchronization::Adapter\"",
    };
    write_package(
        &fixture,
        "clock",
        "1.0.0",
        &format!(
            r#"
[package.metadata.cargo_check_external_types]
allowed_external_types = [{clock_allowlist}]
[dependencies]
coordinate_facade = {{ path = "../coordinate_facade", version = "1.0.0" }}
synchronization = {{ path = "../synchronization", version = "1.0.0" }}
"#
        ),
    );
    let mut clock = "
use coordinate_facade::{Thread, ThreadAware};
pub struct Clock { timers: synchronization::Mutex<usize> }
impl Clock {
    pub fn new() -> Self { Self { timers: synchronization::Mutex::new(0) } }
}
impl ThreadAware for Clock {
    fn relocate(&mut self, destination: &Thread) { self.timers.relocate(destination); }
}
"
    .to_owned();
    match exposure {
        Exposure::SharedCore => {}
        Exposure::Synchronization => clock.push_str(
            "
impl Clock {
    pub fn mutex(&self) -> &synchronization::Mutex<usize> { &self.timers }
}
",
        ),
        Exposure::NestedAdapter => "pub use synchronization::Adapter;\n".clone_into(&mut clock),
    }
    fixture.write("packages/clock/src/lib.rs", &clock);
    write_package(
        &fixture,
        "consumer",
        "1.0.0",
        r#"
publish = false
[dependencies]
clock = { path = "../clock", version = "1.0.0" }
coordinate_facade = { path = "../coordinate_facade", version = "1.0.0" }
"#,
    );
    let consumer = if matches!(exposure, Exposure::NestedAdapter) {
        "pub fn consume() { clock::Adapter.accept(&coordinate_facade::Thread); }\n"
    } else {
        "
use coordinate_facade::{Thread, ThreadAware};
fn relocate(clock: &mut impl ThreadAware, thread: &Thread) { clock.relocate(thread); }
pub fn consume() { relocate(&mut clock::Clock::new(), &Thread); }
"
    };
    fixture.write("packages/consumer/src/lib.rs", consumer);
    fixture
}
