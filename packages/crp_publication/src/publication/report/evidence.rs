use std::collections::BTreeSet;
use std::path::Path;

use crp_workspace::identity::immutable_commit;
use ohno::AppError;
use semver::Version;
use serde_json::Value;

use crate::publication::binaries::publish::{BINARY_OUTCOME_SCHEMA_VERSION, BinaryOutcome};
use crate::publication::context::WorkflowRun;
use crate::publication::github::{
    GITHUB_OUTCOME_SCHEMA_VERSION, GithubOutcome, GithubState, PLATFORM_BATCH_SCHEMA_VERSION,
    PlatformBatch,
};
use crate::publication::manifest::{Package, PublicationManifest};
use crate::publication::registry::{OUTCOME_SCHEMA_VERSION, RegistryOutcome, RegistryState};
use crate::publication::report::{ExpectedBatch, Receipt};

/// Decodes complete phase schemas before projecting their validated evidence for selection.
pub(crate) fn parse_receipt(
    bytes: &[u8],
    publication: Option<&PublicationManifest>,
    path: &Path,
) -> Result<Option<Receipt>, AppError> {
    let value: Value = serde_json::from_slice(bytes)?;
    let publication_id = value
        .get("publication_id")
        .and_then(Value::as_str)
        .ok_or_else(|| InvalidReceipt::new("missing publication identity"))?;
    // Only the minimal envelope is needed to discard an unrelated retained outcome.
    // A malformed envelope is still an explicit collection error, never inferred absence.
    let Some(publication) = applicable(publication, publication_id) else {
        return Ok(None);
    };
    let phase = value
        .get("phase")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match phase {
        "registry" => {
            let outcome: RegistryOutcome = serde_json::from_value(value)?;
            require(
                outcome.schema_version == OUTCOME_SCHEMA_VERSION,
                "registry schema",
            )?;
            // Registry initialization records every request before any fallible acquisition.
            inventory(
                publication,
                outcome
                    .packages
                    .iter()
                    .map(|item| (&item.name, &item.version)),
                true,
            )?;
            require(
                outcome
                    .packages
                    .iter()
                    .all(|package| registry_state_matches_mode(&package.state, outcome.dry_run)),
                "registry execution mode",
            )?;
            require(
                outcome.complete == (!outcome.dry_run && outcome.passed()),
                "registry completion",
            )?;
            let summaries = outcome
                .packages
                .iter()
                .map(|item| {
                    format!(
                        "Registry {}@{}: {}.",
                        item.name,
                        item.version,
                        registry_state_description(&item.state)
                    )
                })
                .chain(outcome.notes.iter().cloned())
                .collect();
            Ok(Some(Receipt {
                path: path.to_path_buf(),
                publication_id: outcome.publication_id,
                phase: outcome.phase,
                complete: outcome.complete,
                github: require_workflow_attribution(outcome.github)?,
                target: None,
                batch_id: None,
                batches: Vec::new(),
                errors: outcome.errors,
                summaries,
            }))
        }
        "github" => {
            let outcome: GithubOutcome = serde_json::from_value(value)?;
            require(
                outcome.schema_version == GITHUB_OUTCOME_SCHEMA_VERSION,
                "GitHub schema",
            )?;
            // GitHub records are appended during reconciliation; an early failed attempt can
            // legitimately contain a partial inventory, but a complete outcome cannot.
            inventory(
                publication,
                outcome
                    .packages
                    .iter()
                    .map(|item| (&item.name, &item.version)),
                outcome.complete,
            )?;
            require(
                outcome.complete
                    == (!outcome.dry_run && outcome.passed(publication.publication.packages.len())),
                "GitHub completion",
            )?;
            let mut summaries = Vec::new();
            for item in &outcome.packages {
                require(
                    item.tag == format!("{}-v{}", item.name, item.version),
                    "package tag",
                )?;
                require(
                    item.source.as_deref().is_none_or(immutable_commit),
                    "tag source",
                )?;
                require(
                    item.observed_version
                        .as_deref()
                        .is_none_or(|version| Version::parse(version).is_ok()),
                    "observed version",
                )?;
                require(
                    item.recovery_source.as_ref().is_none_or(|source| {
                        *source == publication.publication.source
                            && item.state == GithubState::Failed
                    }),
                    "tag recovery source",
                )?;
                require(
                    item.state != GithubState::Complete || item.source.is_some(),
                    "completed tag source",
                )?;
                require(
                    outcome.dry_run
                        || !matches!(
                            item.state,
                            GithubState::WouldCreateTag | GithubState::WouldCreateRelease
                        ),
                    "GitHub execution mode",
                )?;
                summaries.push(format!(
                    "GitHub {}@{}: {}.",
                    item.name,
                    item.version,
                    github_state_description(item.state)
                ));
                if let Some(version) = &item.observed_version {
                    summaries.push(format!(
                        "{} requested {}; release branch observed {}.",
                        item.name, item.version, version
                    ));
                }
                if let Some(source) = &item.recovery_source {
                    summaries.push(format!("Missing tag {}: verify original publication source {source}, create only this missing tag there, and retry the original workflow. Do not move an existing tag.", item.tag));
                }
            }
            let planned: BTreeSet<_> = outcome.planned_targets.iter().collect();
            require(
                planned.len() == outcome.planned_targets.len(),
                "duplicate planned target",
            )?;
            for target in &planned {
                require(
                    publication.publication.packages.iter().any(|package| {
                        package.binary.as_ref().is_some_and(|binary| {
                            binary
                                .targets
                                .iter()
                                .any(|candidate| candidate.triple() == target.as_str())
                        })
                    }),
                    "unrequested planned target",
                )?;
            }
            let mut targets = BTreeSet::new();
            for batch in &outcome.batches {
                require(targets.insert(&batch.target), "duplicate batch target")?;
                require(planned.contains(&batch.target), "unplanned batch")?;
                require(is_batch_id(&batch.batch_id), "batch identity")?;
                require(
                    batch.path == format!("{}.json", batch.target),
                    "batch routing path",
                )?;
            }
            require(
                !outcome.dry_run || outcome.batches.is_empty(),
                "dry-run batch artifacts",
            )?;
            require(
                outcome.dry_run || targets == planned,
                "missing planned batch artifact",
            )?;
            Ok(Some(Receipt {
                path: path.to_path_buf(),
                publication_id: outcome.publication_id,
                phase: outcome.phase,
                complete: outcome.complete,
                github: require_workflow_attribution(outcome.github)?,
                target: None,
                batch_id: None,
                batches: outcome
                    .batches
                    .into_iter()
                    .map(|batch| ExpectedBatch {
                        target: batch.target,
                        batch_id: batch.batch_id,
                    })
                    .collect(),
                errors: outcome.errors,
                summaries,
            }))
        }
        "binaries" => binary_receipt(value, publication, path),
        _ => Err(InvalidReceipt::new("unknown phase").into()),
    }
}

fn applicable<'a>(
    publication: Option<&'a PublicationManifest>,
    id: &str,
) -> Option<&'a PublicationManifest> {
    publication.filter(|publication| publication.id == id)
}

fn require_workflow_attribution(context: Option<WorkflowRun>) -> Result<WorkflowRun, AppError> {
    context.ok_or_else(|| InvalidReceipt::new("missing workflow attribution").into())
}

fn require(valid: bool, field: &'static str) -> Result<(), AppError> {
    if valid {
        Ok(())
    } else {
        Err(InvalidReceipt::new(field).into())
    }
}

// PlatformBatch::identity emits lowercase SHA-256 hex for the current batch schema.
// This is shape validation only; binary outcome validation reproduces the actual identity.
fn is_batch_id(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn inventory<'a>(
    publication: &PublicationManifest,
    items: impl Iterator<Item = (&'a String, &'a String)>,
    complete: bool,
) -> Result<(), AppError> {
    let mut names = BTreeSet::new();
    for (name, version) in items {
        require(names.insert(name), "duplicate package evidence")?;
        requested(publication, name, version)?;
    }
    require(
        !complete || names.len() == publication.publication.packages.len(),
        "missing package evidence",
    )
}

fn requested<'a>(
    publication: &'a PublicationManifest,
    name: &str,
    version: &str,
) -> Result<&'a Package, AppError> {
    publication
        .publication
        .packages
        .iter()
        .find(|package| package.name == name && package.version == version)
        .ok_or_else(|| InvalidReceipt::new("unrequested package identity").into())
}

fn binary_receipt(
    value: Value,
    publication: &PublicationManifest,
    path: &Path,
) -> Result<Option<Receipt>, AppError> {
    let outcome: BinaryOutcome = serde_json::from_value(value)?;
    require(
        outcome.schema_version == BINARY_OUTCOME_SCHEMA_VERSION,
        "binary schema",
    )?;
    require(!outcome.items.is_empty(), "empty binary receipt")?;
    let mut names = BTreeSet::new();
    let mut binaries = Vec::new();
    let mut summaries = Vec::new();
    let mut passed = true;
    for item in &outcome.items {
        let binary = &item.binary;
        binary.validate()?;
        let package = requested(publication, &binary.name, &binary.version)?;
        require(names.insert(&binary.name), "duplicate binary evidence")?;
        require(
            package.binary.as_ref().is_some_and(|request| {
                request.name == binary.bin
                    && request
                        .targets
                        .iter()
                        .any(|target| target.triple() == outcome.target)
            }),
            "unrequested binary target",
        )?;
        let success = match (item.status.as_str(), item.stage.as_str()) {
            ("published", "upload") | ("skipped-complete", "refresh") if !outcome.no_upload => true,
            ("staged-only", "package") if outcome.no_upload => true,
            ("failed", "refresh" | "source" | "build" | "package" | "upload" | "cancelled") => {
                require(
                    item.diagnostic
                        .as_ref()
                        .is_some_and(|text| !text.is_empty()),
                    "missing item failure diagnostic",
                )?;
                false
            }
            ("unattempted", "cancelled") => false,
            _ => return Err(InvalidReceipt::new("binary status or stage").into()),
        };
        require(
            !success || item.diagnostic.is_none(),
            "successful item failure diagnostic",
        )?;
        passed &= success && item.cleanup_error.is_none();
        summaries.push(format!(
            "Binary {}@{} [{}]: {} during {}.",
            binary.name, binary.version, outcome.target, item.status, item.stage
        ));
        if let Some(diagnostic) = &item.diagnostic {
            summaries.push(format!("{}: {diagnostic}", binary.tag));
        }
        if let Some(cleanup) = &item.cleanup_error {
            summaries.push(format!("{} source cleanup: {cleanup}", binary.tag));
        }
        binaries.push(binary.clone());
    }
    require(
        outcome.complete == (passed && !outcome.no_upload),
        "binary completion",
    )?;
    // Reconciliation emits package-sorted batches; execution groups or skips items independently.
    // Restore that producer order to verify the complete item inventory against its frozen ID.
    binaries.sort_by(|left, right| left.name.cmp(&right.name));
    let batch = PlatformBatch {
        schema_version: PLATFORM_BATCH_SCHEMA_VERSION,
        publication_id: publication.id.clone(),
        repository: publication
            .publication
            .configuration
            .repository()
            .to_owned(),
        target: outcome.target.clone(),
        binaries,
        batch_id: outcome.batch_id.clone(),
    };
    batch.verify_identity()?;
    Ok(Some(Receipt {
        path: path.to_path_buf(),
        publication_id: outcome.publication_id,
        phase: outcome.phase,
        complete: outcome.complete,
        github: require_workflow_attribution(outcome.github)?,
        target: Some(outcome.target),
        batch_id: Some(outcome.batch_id),
        batches: Vec::new(),
        errors: Vec::new(),
        summaries,
    }))
}

/// Malformed phase evidence cannot establish delivery or authorize recovery instructions.
#[ohno::error]
#[display("invalid publication receipt: {field}")]
struct InvalidReceipt {
    field: &'static str,
}

fn registry_state_matches_mode(state: &RegistryState, dry_run: bool) -> bool {
    match state {
        RegistryState::AlreadyPresent | RegistryState::Unknown => true,
        RegistryState::Published | RegistryState::Missing => !dry_run,
        RegistryState::WouldPublish => dry_run,
    }
}

fn registry_state_description(state: &RegistryState) -> &'static str {
    match state {
        RegistryState::AlreadyPresent => "already present",
        RegistryState::Published => "published and available",
        RegistryState::WouldPublish => "would publish",
        RegistryState::Missing => "not available",
        RegistryState::Unknown => "availability unknown",
    }
}

fn github_state_description(state: GithubState) -> &'static str {
    match state {
        GithubState::Pending => "not completed",
        GithubState::Complete => "required reconciliation complete",
        GithubState::WouldCreateTag => "would create tag",
        GithubState::WouldCreateRelease => "would create release",
        GithubState::Failed => "reconciliation failed",
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::indexing_slicing,
    reason = "Tests mutate their fixed JSON fixtures."
)]
mod tests {
    use serde_json::json;

    use super::*;

    fn publication() -> PublicationManifest {
        PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main",
                "targets":["x86_64-unknown-linux-gnu"]},
            "packages":[
                {"name":"first","version":"1.0.0","manifest":"first/Cargo.toml",
                    "binary":{"name":"first","targets":["x86_64-unknown-linux-gnu"]}},
                {"name":"second","version":"1.0.0","manifest":"second/Cargo.toml",
                    "binary":{"name":"second","targets":["x86_64-unknown-linux-gnu"]}}
            ]
        })).unwrap()).unwrap()
    }

    fn registry(publication: &PublicationManifest) -> Value {
        json!({"schema_version":1,"publication_id":publication.id,"phase":"registry",
        "dry_run":false,"complete":true,"errors":[],"notes":[],
        "github":{"run_id":123,"run_attempt":1},
        "packages":[
            {"name":"first","version":"1.0.0","state":"already_present"},
            {"name":"second","version":"1.0.0","state":"published"}
        ]})
    }

    fn github(publication: &PublicationManifest) -> Value {
        json!({"schema_version":1,"publication_id":publication.id,"phase":"github",
        "dry_run":false,"complete":true,"errors":[],"planned_targets":[],"batches":[],
        "github":{"run_id":123,"run_attempt":1},
        "packages":(["first","second"].map(|name| json!({
            "name":name,"version":"1.0.0","tag":format!("{name}-v1.0.0"),
            "state":"complete","source":"b".repeat(40),
            "recovery_source":null,"observed_version":null
        })))})
    }

    fn binary_outcome(publication: &PublicationManifest) -> Value {
        let binaries: Vec<_> = ["first", "second"]
            .map(|name| {
                json!({
                    "name":name,"bin":name,"version":"1.0.0","tag":format!("{name}-v1.0.0"),
                    "source_sha":"b".repeat(40)
                })
            })
            .into_iter()
            .collect();
        let batch: PlatformBatch = serde_json::from_value(json!({
            "schema_version":1,"publication_id":publication.id,"repository":"example/tool",
            "target":"x86_64-unknown-linux-gnu","binaries":binaries,"batch_id":""
        }))
        .unwrap();
        let batch = batch.seal().unwrap();
        json!({
            "schema_version":1,"publication_id":publication.id,"phase":"binaries",
            "target":"x86_64-unknown-linux-gnu","batch_id":batch.batch_id,"no_upload":false,
            "complete":false,"github":{"run_id":123,"run_attempt":1},
            "items":[
                {"binary":binaries[0],"status":"published","stage":"upload","diagnostic":null,"cleanup_error":null},
                {"binary":binaries[1],"status":"failed","stage":"build","diagnostic":"compiler failed","cleanup_error":"cleanup failed"}
            ]
        })
    }

    fn github_with_batch(publication: &PublicationManifest) -> Value {
        let binary = binary_outcome(publication);
        let target = binary["target"].as_str().unwrap();
        let mut value = github(publication);
        value["planned_targets"] = json!([target]);
        value["batches"] = json!([{
            "target":target,"path":format!("{target}.json"),"batch_id":binary["batch_id"]
        }]);
        value
    }

    fn read(value: &Value, publication: &PublicationManifest) -> Result<Receipt, AppError> {
        parse_receipt(
            &serde_json::to_vec(value).unwrap(),
            Some(publication),
            Path::new("outcome.json"),
        )
        .map(|receipt| receipt.unwrap())
    }

    fn assert_invalid(value: &Value, publication: &PublicationManifest) {
        let error = read(value, publication).unwrap_err();
        assert!(error.find_source::<InvalidReceipt>().is_some());
    }

    #[test]
    fn unrelated_stale_schema_does_not_destroy_current_evidence() {
        let publication = publication();
        let value = json!({"publication_id":"unrelated","phase":"obsolete","schema_version":999});
        assert!(
            parse_receipt(
                &serde_json::to_vec(&value).unwrap(),
                Some(&publication),
                Path::new("stale/outcome.json")
            )
            .unwrap()
            .is_none()
        );
        parse_receipt(b"{}", Some(&publication), Path::new("unknown/outcome.json")).unwrap_err();
    }

    #[test]
    fn incomplete_registry_outcomes_still_reject_impossible_execution_modes() {
        let publication = publication();
        for (dry_run, state) in [
            (true, "published"),
            (true, "missing"),
            (false, "would_publish"),
        ] {
            let mut value = registry(&publication);
            value["dry_run"] = json!(dry_run);
            value["complete"] = json!(false);
            value["packages"][0]["state"] = json!(state);
            assert_invalid(&value, &publication);
        }
    }

    #[test]
    fn complete_envelopes_require_the_real_phase_inventory_and_verdict() {
        let publication = publication();
        for value in [registry(&publication), github(&publication)] {
            assert!(read(&value, &publication).unwrap().complete);
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove("packages");
            read(&missing, &publication).unwrap_err();
            let mut empty = value.clone();
            empty["packages"] = json!([]);
            read(&empty, &publication).unwrap_err();
            let mut duplicate = value.clone();
            duplicate["packages"][1] = duplicate["packages"][0].clone();
            read(&duplicate, &publication).unwrap_err();
            let mut contradictory = value.clone();
            contradictory["errors"] = json!(["failure"]);
            read(&contradictory, &publication).unwrap_err();
            let mut dry = value;
            dry["dry_run"] = json!(true);
            read(&dry, &publication).unwrap_err();
        }
    }

    #[test]
    fn superseded_tag_retains_observed_version_recovery_and_independent_success() {
        let publication = publication();
        let mut value = github(&publication);
        value["complete"] = json!(false);
        value["packages"][0]["state"] = json!("failed");
        value["packages"][0]["source"] = Value::Null;
        value["packages"][0]["recovery_source"] = json!(publication.publication.source);
        value["packages"][0]["observed_version"] = json!("2.0.0");
        value["errors"] = json!(["Candidate is not release-equivalent."]);
        let receipt = read(&value, &publication).unwrap();
        assert!(!receipt.complete);
        assert!(receipt.summaries.iter().any(|line| line.contains("2.0.0")));
        assert!(
            receipt
                .summaries
                .iter()
                .any(|line| line.contains(&"a".repeat(40)))
        );
        assert!(
            receipt
                .summaries
                .iter()
                .any(|line| line.contains("second@1.0.0"))
        );
        assert!(
            receipt
                .errors
                .iter()
                .any(|line| line.contains("not release-equivalent"))
        );
    }

    #[test]
    fn binary_items_preserve_partial_results_and_bind_the_complete_inventory() {
        let publication = publication();
        let mut value = binary_outcome(&publication);
        let receipt = read(&value, &publication).unwrap();
        assert!(!receipt.complete);
        for expected in [
            "first@1.0.0",
            "second@1.0.0",
            "compiler failed",
            "cleanup failed",
        ] {
            assert!(receipt.summaries.iter().any(|line| line.contains(expected)));
        }
        value["complete"] = json!(true);
        read(&value, &publication).unwrap_err();
        value["complete"] = json!(false);
        value["items"].as_array_mut().unwrap().pop();
        read(&value, &publication).unwrap_err();
    }

    #[test]
    fn phase_schemas_and_workflow_attribution_are_required_before_selection() {
        let publication = publication();
        for value in [
            registry(&publication),
            github(&publication),
            binary_outcome(&publication),
        ] {
            read(&value, &publication).unwrap();
            let mut unsupported = value.clone();
            unsupported["schema_version"] = json!(2);
            assert_invalid(&unsupported, &publication);
            let mut unattributed = value.clone();
            unattributed["github"] = Value::Null;
            assert_invalid(&unattributed, &publication);

            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(
                parse_receipt(&bytes, None, Path::new("outcome.json"))
                    .unwrap()
                    .is_none()
            );
            let mut unrelated = value;
            unrelated["publication_id"] = json!("another-publication");
            assert!(
                parse_receipt(
                    &serde_json::to_vec(&unrelated).unwrap(),
                    Some(&publication),
                    Path::new("unrelated/outcome.json"),
                )
                .unwrap()
                .is_none()
            );
        }
        assert_invalid(&json!({"phase":"unknown"}), &publication);
    }

    #[test]
    fn github_batch_routes_preserve_the_planned_target_and_frozen_identity() {
        let publication = publication();
        let value = github_with_batch(&publication);
        let receipt = read(&value, &publication).unwrap();
        assert!(receipt.complete);
        assert_eq!(receipt.batches.len(), 1);
        let batch = receipt.batches.first().unwrap();
        assert_eq!(batch.target, value["planned_targets"][0].as_str().unwrap());
        assert_eq!(
            batch.batch_id,
            value["batches"][0]["batch_id"].as_str().unwrap()
        );

        let target = value["planned_targets"][0].clone();
        for (pointer, replacement) in [
            ("/planned_targets", json!([target, target])),
            ("/planned_targets", json!(["x86_64-pc-windows-msvc"])),
            ("/planned_targets", json!([])),
            (
                "/batches",
                json!([value["batches"][0], value["batches"][0]]),
            ),
            ("/batches", json!([])),
            ("/batches/0/batch_id", json!("a".repeat(63))),
            ("/batches/0/batch_id", json!("G".repeat(64))),
            ("/batches/0/path", json!("../another-batch.json")),
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert_invalid(&invalid, &publication);
        }
    }

    #[test]
    fn github_dry_runs_retain_intent_without_emitting_batch_artifacts() {
        let publication = publication();
        let mut value = github_with_batch(&publication);
        value["dry_run"] = json!(true);
        value["complete"] = json!(false);
        assert_invalid(&value, &publication);
        value["batches"] = json!([]);
        value["packages"][0]["state"] = json!("would_create_tag");
        value["packages"][0]["source"] = Value::Null;
        value["packages"][1]["state"] = json!("would_create_release");
        let receipt = read(&value, &publication).unwrap();
        assert!(!receipt.complete);
        assert!(receipt.batches.is_empty());
        assert!(receipt.errors.is_empty());
        assert_eq!(receipt.summaries.len(), 2);

        value["dry_run"] = json!(false);
        value["planned_targets"] = json!([]);
        for state in ["would_create_tag", "would_create_release"] {
            value["packages"][0]["state"] = json!(state);
            assert_invalid(&value, &publication);
        }
    }

    #[test]
    fn github_package_evidence_cannot_change_release_or_recovery_identity() {
        let publication = publication();
        let value = github(&publication);
        for (pointer, replacement) in [
            ("/packages/0/tag", json!("first-v2.0.0")),
            ("/packages/0/source", json!("short")),
            ("/packages/0/source", Value::Null),
            ("/packages/0/observed_version", json!("not-semver")),
            (
                "/packages/0/recovery_source",
                json!(publication.publication.source),
            ),
            ("/packages/0/name", json!("unrequested")),
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert_invalid(&invalid, &publication);
        }
        let mut failed = value;
        failed["complete"] = json!(false);
        failed["packages"][0]["state"] = json!("failed");
        failed["packages"][0]["source"] = Value::Null;
        failed["packages"][0]["recovery_source"] = json!("c".repeat(40));
        assert_invalid(&failed, &publication);
    }

    #[test]
    fn binary_outcomes_require_requested_work_and_consistent_item_evidence() {
        let publication = publication();
        let value = binary_outcome(&publication);
        for (pointer, replacement) in [
            ("/items", json!([])),
            ("/items", json!([value["items"][0], value["items"][0]])),
            ("/items/0/binary/bin", json!("another-executable")),
            ("/target", json!("x86_64-pc-windows-msvc")),
            ("/items/0/stage", json!("build")),
            ("/items/0/status", json!("unknown")),
            ("/items/0/diagnostic", json!("unexpected failure")),
            ("/items/1/diagnostic", Value::Null),
            ("/items/1/diagnostic", json!("")),
            ("/no_upload", json!(true)),
        ] {
            let mut invalid = value.clone();
            *invalid.pointer_mut(pointer).unwrap() = replacement;
            assert_invalid(&invalid, &publication);
        }
    }

    #[test]
    fn binary_modes_and_reordered_results_keep_the_frozen_inventory() {
        let publication = publication();
        for no_upload in [false, true] {
            let mut value = binary_outcome(&publication);
            value["no_upload"] = json!(no_upload);
            value["complete"] = json!(!no_upload);
            for (index, item) in value["items"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .enumerate()
            {
                let (status, stage) = if no_upload {
                    ("staged-only", "package")
                } else if index == 0 {
                    ("published", "upload")
                } else {
                    ("skipped-complete", "refresh")
                };
                item["status"] = json!(status);
                item["stage"] = json!(stage);
                item["diagnostic"] = Value::Null;
                item["cleanup_error"] = Value::Null;
            }
            // Skips and source grouping can reorder execution without changing the batch.
            value["items"].as_array_mut().unwrap().reverse();
            let receipt = read(&value, &publication).unwrap();
            assert_eq!(receipt.complete, !no_upload);
            assert_eq!(receipt.batch_id.as_deref(), value["batch_id"].as_str());
            assert_eq!(receipt.summaries.len(), 2);
        }
        let mut cancelled = binary_outcome(&publication);
        for item in cancelled["items"].as_array_mut().unwrap() {
            item["status"] = json!("unattempted");
            item["stage"] = json!("cancelled");
            item["diagnostic"] = Value::Null;
            item["cleanup_error"] = Value::Null;
        }
        let receipt = read(&cancelled, &publication).unwrap();
        assert!(!receipt.complete);
        assert_eq!(receipt.batch_id.as_deref(), cancelled["batch_id"].as_str());
        assert_eq!(receipt.summaries.len(), 2);
    }
}
