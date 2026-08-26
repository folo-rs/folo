// `check` command: fail on unreleased changes or an inconsistent group.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

use ohno::AppError;

use crate::classify::{Classification, PackageStatus, classify};
use crate::command::run_capture;
use crate::verbose::Verbose;
use crate::{CheckFormat, INCREMENT_VERSIONS_SKILL};

pub(crate) fn run_check(
    base: &str,
    manifest_path: &Path,
    format: CheckFormat,
    verify_packaging: bool,
    verbose: Verbose,
) -> Result<(bool, String), AppError> {
    let classification = classify(manifest_path, base, verbose)?;
    let mut message = render_offences(&classification, format);

    if verify_packaging {
        append_packaging_warnings(&mut message, &verify_packaging_rules(&classification));
    }

    let passed = classification
        .packages
        .iter()
        .all(|package| package.status != PackageStatus::UnreleasedChanges)
        && classification.groups.values().all(|group| group.consistent);

    if let Some(success) = default_success_message(passed, &message) {
        message = success.to_string();
    }

    Ok((passed, message))
}

fn default_success_message(passed: bool, message: &str) -> Option<&'static str> {
    if passed && message.is_empty() {
        Some("All publishable packages are released or releasing.")
    } else {
        None
    }
}

fn render_offences(classification: &Classification, format: CheckFormat) -> String {
    let mut lines = Vec::new();
    for package in &classification.packages {
        if package.status != PackageStatus::UnreleasedChanges {
            continue;
        }
        let anchor = package.anchor.as_ref();
        let anchor_text = match anchor {
            Some(anchor) => format!("{} ({})", short_sha(&anchor.commit), anchor.version),
            None => "no base revision".to_string(),
        };
        let group_text = match &package.group {
            Some(group) => {
                let members = classification
                    .groups
                    .get(group)
                    .map_or_else(|| group.clone(), |verdict| verdict.members.join(", "));
                format!(" Group {group} also includes {members}.")
            }
            None => String::new(),
        };
        let changed = if package.changed.is_empty() {
            "released content changed".to_string()
        } else {
            let first = package
                .changed
                .iter()
                .find_map(|item| item.path.as_deref().or(item.field.as_deref()))
                .unwrap_or("released content");
            format!("{first} (and related paths) changed")
        };
        let skill = format!(
            "Run the {INCREMENT_VERSIONS_SKILL} skill to propose and apply version increments."
        );
        let text = format!(
            "{}: unreleased-changes since {anchor_text}; {changed}.{group_text} {skill}",
            package.name
        );
        if format == CheckFormat::Github {
            let file = package.manifest_path.to_string_lossy().replace('\\', "/");
            lines.push(format!(
                "::error file={file},title=unreleased-changes::{text}"
            ));
        }
        lines.push(text);
    }

    for (name, verdict) in &classification.groups {
        if verdict.consistent {
            continue;
        }
        let listed = verdict.members.join(", ");
        let skill = format!(
            "Run the {INCREMENT_VERSIONS_SKILL} skill to propose and apply version increments."
        );
        let text = format!("group {name}: members declare different versions ({listed}). {skill}");
        if format == CheckFormat::Github {
            lines.push(format!("::error title=inconsistent-group::{text}"));
        }
        lines.push(text);
    }

    lines.join("\n")
}

fn short_sha(commit: &str) -> &str {
    commit.get(..commit.len().min(12)).unwrap_or(commit)
}

// Packaging warnings are non-gating advisory text from `cargo package --list`.
#[cfg_attr(test, mutants::skip)]
fn append_packaging_warnings(message: &mut String, warnings: &str) {
    if warnings.is_empty() {
        return;
    }
    if !message.is_empty() {
        message.push('\n');
    }
    message.push_str(warnings);
}

// Cross-checks against `cargo package --list`; not practical to mutate in unit tests.
#[cfg_attr(test, mutants::skip)]
fn verify_packaging_rules(classification: &Classification) -> String {
    let mut warnings = String::new();
    for package in &classification.work_tree.packages {
        // Non-gating: a packaging probe that cannot run is skipped.
        let Ok(listed) = cargo_package_list(
            &classification.work_tree.workspace_root,
            &package.manifest.name,
        ) else {
            continue;
        };
        let dir = &package.manifest.directory;
        let tool: BTreeSet<String> = classification
            .git
            .ls_files(dir)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|full| {
                let full = crate::git::git_path(&full);
                let rel = crate::packaging::relativize(&full, dir)?;
                package
                    .manifest
                    .packaging
                    .is_released(rel)
                    .then(|| rel.to_string())
            })
            .collect();
        let cargo: BTreeSet<String> = listed
            .into_iter()
            .filter(|path| path != "Cargo.lock" && !path.ends_with("/Cargo.lock"))
            .collect();
        if tool == cargo {
            continue;
        }
        writeln!(
            warnings,
            "warning: packaging rule mismatch for {}: tool and `cargo package --list` differ (Cargo.lock ignored)",
            package.manifest.name
        )
        .expect("writing to String");
    }
    warnings
}

// Spawns `cargo package --list`; catching mutations would compile every fixture.
#[cfg_attr(test, mutants::skip)]
fn cargo_package_list(workspace_root: &Path, package: &str) -> Result<Vec<String>, AppError> {
    let stdout = run_capture(
        "cargo",
        &[
            "package",
            "--list",
            "--offline",
            "--allow-dirty",
            "-p",
            package,
        ],
        workspace_root,
    )?;
    Ok(stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn short_sha_truncates_long_revisions() {
        assert_eq!(short_sha("abcdefghijklmnop"), "abcdefghijkl");
        assert_eq!(short_sha("abc"), "abc");
    }

    #[test]
    fn default_success_message_only_when_passed_and_empty() {
        assert!(default_success_message(true, "").is_some());
        assert!(default_success_message(true, "warning").is_none());
        assert!(default_success_message(false, "").is_none());
        assert!(default_success_message(false, "fail").is_none());
    }
}
