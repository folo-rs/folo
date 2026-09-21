use std::collections::BTreeSet;

use ohno::AppError;
use serde::Deserialize;

/// Projects the completed versioning report into packages that will publish.
///
/// The caller supplies verified evidence; this check does not repeat version planning.
#[derive(Deserialize)]
struct ReleaseReport {
    schema_version: u32,
    packages: Vec<Package>,
}

/// Retains only the package identity and release disposition needed by this consumer.
#[derive(Deserialize)]
struct Package {
    name: String,
    status: Status,
}

/// Mirrors the report's release dispositions without imposing a version increment policy.
#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
enum Status {
    PendingRelease,
    NeedsIncrement,
    Unchanged,
}

/// The action manifest remains the authority for all tool names, including fixture helpers.
#[derive(Deserialize)]
struct ActionManifest {
    schema_version: u32,
    tools: Vec<Tool>,
}

/// Other manifest fields belong to the action's own release validation.
#[derive(Deserialize)]
struct Tool {
    name: String,
}

/// Returns whether any pending release is pinned by the action.
///
/// Loading the manifest is deferred until a nonempty release set exists, so documentation-only
/// and nonpublished-helper changes need neither GitHub access nor an action checkout.
pub(crate) fn pairing_needed(
    report: &str,
    manifest: impl FnOnce() -> Result<String, AppError>,
    mut note: impl FnMut(&str),
) -> Result<bool, AppError> {
    // These are the formats consumed from cargo-release-plan and the action repository.
    const REPORT_SCHEMA: u32 = 4;
    const MANIFEST_SCHEMA: u32 = 1;

    let report: ReleaseReport = serde_json::from_str(report)
        .map_err(|error| DecodeFailed::caused_by("release report", error))?;
    if report.schema_version != REPORT_SCHEMA {
        return Err(InvalidInput::new("Unsupported release report schema").into());
    }
    let mut seen = BTreeSet::new();
    let mut pending = BTreeSet::new();
    for package in report.packages {
        if !valid_package_name(&package.name) || !seen.insert(package.name.clone()) {
            return Err(
                InvalidInput::new("Release report has an empty or duplicate package name").into(),
            );
        }
        match package.status {
            Status::PendingRelease => {
                pending.insert(package.name);
            }
            Status::NeedsIncrement => {
                return Err(InvalidInput::new(
                    "Complete version planning before checking action pairing",
                )
                .into());
            }
            Status::Unchanged => {}
        }
    }
    if pending.is_empty() {
        note("The verified report has no pending releases; no action manifest lookup is needed.");
        return Ok(false);
    }

    note(&format!(
        "Pending release candidates: {pending:?}. Reading the authoritative action tool list."
    ));
    let manifest: ActionManifest = serde_json::from_str(&manifest()?)
        .map_err(|error| DecodeFailed::caused_by("action release manifest", error))?;
    if manifest.schema_version != MANIFEST_SCHEMA {
        return Err(InvalidInput::new("Unsupported action release manifest schema").into());
    }
    let mut tools = BTreeSet::new();
    for tool in manifest.tools {
        if !valid_package_name(&tool.name) || !tools.insert(tool.name) {
            return Err(
                InvalidInput::new("Action manifest has an empty or duplicate tool name").into(),
            );
        }
    }
    if tools.is_empty() {
        return Err(InvalidInput::new("Action manifest contains no pinned tools").into());
    }
    let matched: Vec<_> = pending.intersection(&tools).collect();
    note(&format!(
        "Pending packages also pinned by the action: {matched:?}."
    ));
    Ok(!matched.is_empty())
}

/// Keeps malformed names from turning a real package match into a false no-op.
fn valid_package_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// Malformed evidence cannot authorize skipping repository-specific release work.
#[ohno::error]
#[display("{reason}")]
struct InvalidInput {
    reason: &'static str,
}

/// Identifies which serialized input failed while preserving the decoder's diagnostic.
#[ohno::error]
#[display("Cannot decode {kind}")]
struct DecodeFailed {
    kind: &'static str,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn report(packages: &[(&str, &str)]) -> String {
        json!({
            "schema_version": 4,
            "packages": packages.iter().map(|(name, status)| {
                json!({"name": name, "status": status})
            }).collect::<Vec<_>>()
        })
        .to_string()
    }

    fn manifest(names: &[&str]) -> String {
        json!({
            "schema_version": 1,
            "tools": names.iter().map(|name| json!({"name": name})).collect::<Vec<_>>()
        })
        .to_string()
    }

    #[test]
    fn an_empty_release_set_never_loads_the_manifest() {
        let result = pairing_needed(
            &report(&[("unrelated", "unchanged")]),
            || panic!("an unrelated no-release change must not need GitHub"),
            |_| {},
        )
        .unwrap();
        assert!(!result);
    }

    #[test]
    fn unrelated_releases_do_not_require_pairing() {
        let result = pairing_needed(
            &report(&[("unrelated", "pending-release"), ("tool", "unchanged")]),
            || Ok(manifest(&["tool"])),
            |_| {},
        )
        .unwrap();
        assert!(!result);
    }

    #[test]
    fn every_manifest_tool_role_counts_including_retained_and_new_releases() {
        for name in ["tool", "fixture", "future-tool"] {
            let result = pairing_needed(
                &report(&[(name, "pending-release")]),
                || Ok(manifest(&["tool", "fixture", "future-tool"])),
                |_| {},
            )
            .unwrap();
            assert!(result);
        }
    }

    #[test]
    fn malformed_reports_and_unfinished_plans_fail_before_lookup() {
        for invalid in [
            "{}".to_owned(),
            r#"{"schema_version":3,"packages":[]}"#.to_owned(),
            report(&[("tool", "needs-increment")]),
            report(&[("tool", "unknown")]),
            report(&[("", "pending-release")]),
            report(&[("tool ", "pending-release")]),
            report(&[("tool", "unchanged"), ("tool", "pending-release")]),
        ] {
            _ = pairing_needed(&invalid, || panic!("invalid report reached lookup"), |_| {})
                .unwrap_err();
        }
    }

    #[test]
    fn missing_or_invalid_manifests_are_not_false_results() {
        for invalid in [
            "{}".to_owned(),
            r#"{"schema_version":2,"tools":[{"name":"tool"}]}"#.to_owned(),
            manifest(&[]),
            manifest(&[""]),
            manifest(&["tool "]),
            manifest(&["tool", "tool"]),
        ] {
            _ = pairing_needed(
                &report(&[("tool", "pending-release")]),
                || Ok(invalid),
                |_| {},
            )
            .unwrap_err();
        }
        _ = pairing_needed(
            &report(&[("tool", "pending-release")]),
            || Err(InvalidInput::new("manifest lookup canary").into()),
            |_| {},
        )
        .unwrap_err();
    }

    #[test]
    fn diagnostics_explain_candidates_and_matches() {
        let mut notes = Vec::new();
        pairing_needed(
            &report(&[("tool", "pending-release")]),
            || Ok(manifest(&["tool"])),
            |message| notes.push(message.to_owned()),
        )
        .unwrap();
        assert!(notes.iter().all(|message| message.contains("tool")));
    }
}
