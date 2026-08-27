// `check` command: fail on unreleased changes or an inconsistent group.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::Path;

use ohno::AppError;
use semver::Version;

use crate::classify::{ChangedItem, Classification, PackageClass, PackageStatus, classify};
use crate::command::run_capture;
use crate::git::git_path;
use crate::groups::GroupVerdict;
use crate::packaging::relativize;
use crate::verbose::Verbose;
use crate::{CheckFormat, INCREMENT_VERSIONS_SKILL, short_commit};

pub(crate) fn run_check(
    base: &str,
    manifest_path: &Path,
    format: CheckFormat,
    verify_packaging: bool,
    verbose: Verbose,
) -> Result<(bool, String), AppError> {
    let classification = classify(manifest_path, base, verbose)?;
    let mut message = render_diagnostics(&classification.packages, &classification.groups, format);

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

/// Renders the failing packages and groups as human- or workflow-readable text.
///
/// Takes the classified packages and group verdicts rather than the whole
/// [`Classification`] because the rendering depends on nothing else, and the
/// remainder carries the Git repository and work tree that a caller would
/// otherwise have to build.
fn render_diagnostics(
    packages: &[PackageClass],
    groups: &BTreeMap<String, GroupVerdict>,
    format: CheckFormat,
) -> String {
    let declared: BTreeMap<&str, &Version> = packages
        .iter()
        .map(|package| (package.name.as_str(), &package.declared_version))
        .collect();
    let mut lines = Vec::new();
    for package in packages {
        if package.status != PackageStatus::UnreleasedChanges {
            continue;
        }
        let anchor = match package.anchor.as_ref() {
            Some(anchor) => format!("{} ({})", short_commit(&anchor.commit), anchor.version),
            None => "no base revision".to_string(),
        };
        let group_text = match &package.group {
            Some(group) => {
                let members = groups
                    .get(group)
                    .map_or_else(|| group.clone(), |verdict| verdict.members.join(", "));
                format!(" Group {group} also includes {members}.")
            }
            None => String::new(),
        };
        let changed = match package.changed.first() {
            Some(ChangedItem::Package { path, .. }) => {
                format!("{path} (and related paths) changed")
            }
            Some(ChangedItem::Inherited { field }) => {
                format!("{field} (and related paths) changed")
            }
            None => "released content changed".to_string(),
        };
        let text = format!(
            "{}: unreleased-changes since {anchor}; {changed}.{group_text} {}",
            package.name,
            remedy()
        );
        if format == CheckFormat::Github {
            let file = package.manifest_path.to_string_lossy().replace('\\', "/");
            lines.push(format!(
                "::error file={},title=unreleased-changes::{}",
                escape_property(&file),
                escape_data(&text)
            ));
        }
        lines.push(text);
    }

    for (name, verdict) in groups {
        if verdict.consistent {
            continue;
        }
        // Naming the version each member declares is what makes the mismatch
        // actionable; a bare member list only restates the group definition.
        let listed = verdict
            .members
            .iter()
            .map(|member| match declared.get(member.as_str()) {
                Some(version) => format!("{member}@{version}"),
                None => member.clone(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        let text = format!(
            "group {name}: members declare different versions ({listed}). {}",
            remedy()
        );
        if format == CheckFormat::Github {
            lines.push(format!(
                "::error title=inconsistent-group::{}",
                escape_data(&text)
            ));
        }
        lines.push(text);
    }

    lines.join("\n")
}

/// Escapes the message body of a GitHub workflow command.
///
/// A workflow command ends at the first newline and treats `%` as the escape
/// introducer, so a message carrying either would be truncated or would let
/// repository-controlled text start a second command. GitHub's own toolkit
/// applies exactly these three replacements.
fn escape_data(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace('\r', "%0D")
        .replace('\n', "%0A")
}

/// Escapes a property value of a GitHub workflow command.
///
/// Property values are additionally delimited by `:` and `,`, so a path
/// containing either would otherwise split into further properties.
fn escape_property(value: &str) -> String {
    escape_data(value).replace(':', "%3A").replace(',', "%2C")
}

/// The remediation sentence appended to every gating diagnostic.
///
/// The self-contained path comes first so the message stays actionable without
/// any tooling beyond this binary; the skill is named as the assisted route.
fn remedy() -> String {
    format!(
        "Run `cargo release-plan report --out-dir <dir>` to inspect the changes, then \
         `cargo release-plan apply --plan <plan.json>` with an increment plan, or run the \
         {INCREMENT_VERSIONS_SKILL} skill to do both."
    )
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
        let listed = match cargo_package_list(
            &classification.work_tree.workspace_root,
            &package.manifest.name,
        ) {
            Ok(listed) => listed,
            Err(error) => {
                writeln!(
                    warnings,
                    "warning: packaging probe failed for {}: {error}",
                    package.manifest.name
                )
                .expect("writing to String");
                continue;
            }
        };
        let dir = &package.manifest.directory;
        let tracked = match classification.git.ls_files(dir) {
            Ok(files) => files,
            Err(error) => {
                writeln!(
                    warnings,
                    "warning: listing tracked files failed for {}: {error}",
                    package.manifest.name
                )
                .expect("writing to String");
                continue;
            }
        };
        let tool: BTreeSet<String> = tracked
            .into_iter()
            .filter_map(|full| {
                let full = git_path(&full);
                let rel = relativize(&full, dir)?;
                package
                    .manifest
                    .packaging
                    .is_released(rel)
                    .then(|| rel.to_string())
            })
            .collect();
        let cargo: BTreeSet<String> = listed
            .into_iter()
            .filter(|path| !is_packaging_artifact(path))
            .collect();
        if tool == cargo {
            continue;
        }
        // Naming the paths is what makes the warning actionable: the reader has
        // to decide whether a relevance rule is wrong or the tree simply is not
        // clean, and only the differing paths distinguish those.
        let only_in_tool = difference_text(&tool, &cargo);
        let only_in_cargo = difference_text(&cargo, &tool);
        writeln!(
            warnings,
            "warning: packaging rule mismatch for {}: only in tool: {only_in_tool}; \
             only in `cargo package --list`: {only_in_cargo} (Cargo.lock ignored)",
            package.manifest.name
        )
        .expect("writing to String");
    }
    warnings
}

/// Renders the paths in `left` that `right` does not have.
fn difference_text(left: &BTreeSet<String>, right: &BTreeSet<String>) -> String {
    let paths: Vec<&str> = left.difference(right).map(String::as_str).collect();
    if paths.is_empty() {
        "nothing".to_string()
    } else {
        paths.join(", ")
    }
}

/// Whether `cargo package --list` produced this entry rather than the package source.
///
/// The list mixes the crate's own files with entries Cargo synthesizes while
/// packing: it always writes a lockfile into the `.crate`, records the VCS state
/// in `.cargo_vcs_info.json`, and preserves the pre-normalization manifest as
/// `Cargo.toml.orig`. None of those exist in the work tree, so comparing them
/// against the tool's released-content set would report a mismatch on every
/// package. The lockfile in particular is excluded by design because it is
/// derived at pack time rather than being a function of the package source; only
/// the package-root path is synthesized, so a lockfile nested deeper stays in
/// the comparison as the ordinary source file it is.
/// Ref: docs/design.md, "Released content".
fn is_packaging_artifact(path: &str) -> bool {
    path == "Cargo.lock" || path == ".cargo_vcs_info.json" || path == "Cargo.toml.orig"
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
    use std::path::PathBuf;

    use super::*;
    use crate::anchor::Anchor;
    use crate::classify::DiffStat;

    #[test]
    fn default_success_message_only_when_passed_and_empty() {
        assert!(default_success_message(true, "").is_some());
        assert!(default_success_message(true, "warning").is_none());
        assert!(default_success_message(false, "").is_none());
        assert!(default_success_message(false, "fail").is_none());
    }

    /// Builds a package that renders a diagnostic, with the rest left inert.
    ///
    /// Only status, name, anchor, group, and changed items reach the rendered
    /// text, so every other field carries a value the assertions never observe.
    fn failing(name: &str, changed: Vec<ChangedItem>) -> PackageClass {
        PackageClass {
            name: name.to_string(),
            declared_version: Version::new(0, 1, 0),
            group: None,
            status: PackageStatus::UnreleasedChanges,
            anchor: Some(Anchor {
                commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
                version: Version::new(0, 1, 0),
            }),
            changed,
            stat: DiffStat {
                files: 0,
                insertions: 0,
                deletions: 0,
            },
            patch: String::new(),
            untracked: Vec::new(),
            dependencies: Vec::new(),
            dependents: Vec::new(),
            manifest_path: PathBuf::from("packages/demo/Cargo.toml"),
        }
    }

    #[test]
    fn released_packages_produce_no_diagnostics() {
        let mut package = failing("demo", Vec::new());
        package.status = PackageStatus::Released;

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert_eq!(text, "");
    }

    #[test]
    fn a_package_without_an_anchor_says_so() {
        let mut package = failing("demo", Vec::new());
        package.anchor = None;

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert!(text.contains("since no base revision"), "{text}");
    }

    #[test]
    fn a_changed_path_is_named_ahead_of_an_inherited_field() {
        let package = failing(
            "demo",
            vec![
                ChangedItem::Package {
                    path: "src/lib.rs".to_string(),
                    change: "modified".to_string(),
                },
                ChangedItem::Inherited {
                    field: "package.rust-version".to_string(),
                },
            ],
        );

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert!(
            text.contains("src/lib.rs (and related paths) changed"),
            "{text}"
        );
    }

    #[test]
    fn an_inherited_field_is_named_when_it_is_the_only_change() {
        let package = failing(
            "demo",
            vec![ChangedItem::Inherited {
                field: "package.rust-version".to_string(),
            }],
        );

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert!(
            text.contains("package.rust-version (and related paths) changed"),
            "{text}"
        );
    }

    /// A package can reach `unreleased-changes` through an inherited value
    /// alone, which leaves no changed path to name.
    #[test]
    fn a_package_without_changed_items_still_reports() {
        let package = failing("demo", Vec::new());

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert!(text.contains("released content changed"), "{text}");
    }

    #[test]
    fn a_grouped_package_names_the_other_members() {
        let mut package = failing("demo", Vec::new());
        package.group = Some("g".to_string());
        let groups = BTreeMap::from([(
            "g".to_string(),
            GroupVerdict {
                members: vec!["demo".to_string(), "sibling".to_string()],
                consistent: true,
                version: Some(Version::new(0, 1, 0)),
            },
        )]);

        let text = render_diagnostics(&[package], &groups, CheckFormat::Text);

        assert!(
            text.contains("Group g also includes demo, sibling."),
            "{text}"
        );
    }

    /// Group membership is read from the manifest while verdicts are computed
    /// only for groups that have publishable members, so a package can name a
    /// group that has no verdict.
    #[test]
    fn a_grouped_package_without_a_verdict_falls_back_to_the_group_name() {
        let mut package = failing("demo", Vec::new());
        package.group = Some("g".to_string());

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Text);

        assert!(text.contains("Group g also includes g."), "{text}");
    }

    #[test]
    fn an_inconsistent_group_names_the_version_each_member_declares() {
        let mut member = failing("demo", Vec::new());
        member.status = PackageStatus::Releasing;
        member.declared_version = Version::new(0, 2, 0);
        let groups = BTreeMap::from([(
            "g".to_string(),
            GroupVerdict {
                // `absent` is never published, so it has no declared version to
                // report and must still appear in the member list.
                members: vec!["demo".to_string(), "absent".to_string()],
                consistent: false,
                version: None,
            },
        )]);

        let text = render_diagnostics(&[member], &groups, CheckFormat::Text);

        assert!(text.contains("demo@0.2.0"), "{text}");
        assert!(text.contains("absent"), "{text}");
        assert!(!text.contains("absent@"), "{text}");
    }

    #[test]
    fn github_format_precedes_each_diagnostic_with_an_annotation() {
        let package = failing("demo", Vec::new());

        let text = render_diagnostics(&[package], &BTreeMap::new(), CheckFormat::Github);

        let mut lines = text.lines();
        assert!(
            lines
                .next()
                .expect("a failing package renders at least one line")
                .starts_with("::error file=packages/demo/Cargo.toml,title=unreleased-changes::"),
            "{text}"
        );
        assert!(
            lines
                .next()
                .expect("the annotation is followed by the plain diagnostic")
                .starts_with("demo: unreleased-changes"),
            "{text}"
        );
    }

    #[test]
    fn packaging_differences_name_the_paths_on_each_side() {
        let tool = BTreeSet::from(["src/lib.rs".to_string(), "Cargo.toml".to_string()]);
        let cargo = BTreeSet::from(["Cargo.toml".to_string(), "src/extra.rs".to_string()]);

        assert_eq!(difference_text(&tool, &cargo), "src/lib.rs");
        assert_eq!(difference_text(&cargo, &tool), "src/extra.rs");
        assert_eq!(difference_text(&tool, &tool), "nothing");
    }

    #[test]
    fn packaging_warnings_are_appended_below_existing_text() {
        let mut message = "existing".to_string();
        append_packaging_warnings(&mut message, "");
        assert_eq!(message, "existing");

        append_packaging_warnings(&mut message, "warning: x");
        assert_eq!(message, "existing\nwarning: x");

        let mut empty = String::new();
        append_packaging_warnings(&mut empty, "warning: y");
        assert_eq!(empty, "warning: y");
    }

    #[test]
    fn packaging_artifacts_are_ignored_in_verify() {
        assert!(is_packaging_artifact("Cargo.lock"));
        // Only the package-root lockfile is synthesized at pack time.
        assert!(!is_packaging_artifact("fixtures/Cargo.lock"));
        assert!(is_packaging_artifact(".cargo_vcs_info.json"));
        assert!(is_packaging_artifact("Cargo.toml.orig"));
        assert!(!is_packaging_artifact("src/lib.rs"));
    }

    #[test]
    fn workflow_command_data_is_escaped() {
        assert_eq!(escape_data("100% done"), "100%25 done");
        assert_eq!(escape_data("a\r\nb"), "a%0D%0Ab");
        // A colon or comma is only a delimiter in a property, not in the body.
        assert_eq!(escape_data("a:b,c"), "a:b,c");
    }

    #[test]
    fn workflow_command_properties_escape_their_delimiters() {
        assert_eq!(
            escape_property("odd,name/100%/a:b/Cargo.toml"),
            "odd%2Cname/100%25/a%3Ab/Cargo.toml"
        );
    }
}
