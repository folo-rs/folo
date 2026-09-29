// Semantic decision parsing and translation to the shared mechanical version algebra.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crp_diag::{NoteSink, Quotable as _, quote_path};
use ohno::AppError;
use semver::Version;
use serde::{Deserialize, Serialize};

use crate::ReadFileError;
use crate::plan::{PlanIncrement, VersionBump, increment_version};
use crate::propose::generate::Proposal;

/// Local decision-file revision used by the increment-versions skill.
pub const DECISION_SCHEMA_VERSION: u32 = 2;

/// Human semantic assessments, before resolving their mechanical consequences.
///
/// Top-level metadata is extensible; individual decisions deliberately accept only name/impact.
#[derive(Debug, Deserialize)]
pub(crate) struct Decisions {
    schema_version: u32,
    changes: Vec<Change>,
}

impl Decisions {
    #[cfg(test)]
    #[cfg_attr(coverage_nightly, coverage(off))]
    pub(crate) fn for_test(changes: &[(&str, &str)]) -> Self {
        Self {
            schema_version: DECISION_SCHEMA_VERSION,
            changes: changes
                .iter()
                .map(|(name, impact)| Change {
                    name: (*name).to_owned(),
                    impact: match *impact {
                        "breaking" => SemanticImpact::Breaking,
                        "nonbreaking" => SemanticImpact::Nonbreaking,
                        "patch" => SemanticImpact::Patch,
                        _ => panic!("unsupported test change impact"),
                    },
                })
                .collect(),
        }
    }

    pub(crate) fn read(path: &Path) -> Result<Self, AppError> {
        let text =
            fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error))?;
        Self::parse(&text)
    }

    pub(crate) fn parse(text: &str) -> Result<Self, AppError> {
        serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|error| InvalidDecisionFile::caused_by(error).into())
    }

    pub(crate) fn validate(&self) -> Result<BTreeMap<String, SemanticImpact>, AppError> {
        if self.schema_version != DECISION_SCHEMA_VERSION {
            return Err(UnsupportedDecisionSchema::new(self.schema_version).into());
        }
        let mut impacts = BTreeMap::new();
        for change in &self.changes {
            if change.name.trim().is_empty()
                || impacts.insert(change.name.clone(), change.impact).is_some()
            {
                return Err(InvalidDecisionName::new(&change.name).into());
            }
        }
        Ok(impacts)
    }
}

/// One case-sensitive semantic judgement, not an exact version or mechanical bump.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Change {
    name: String,
    impact: SemanticImpact,
}

/// Compatibility significance relative to a package's published anchor.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SemanticImpact {
    Breaking,
    Nonbreaking,
    Patch,
}

impl SemanticImpact {
    pub(crate) fn minimum(self, anchor: &Version) -> Result<Version, AppError> {
        // Cargo's leftmost nonzero component determines compatibility. In particular every
        // 0.0.z movement is breaking, so even a breaking judgement only advances its patch.
        // Ref: packages/cargo-release-plan/docs/design.md, "Public dependencies".
        let bump = match self {
            Self::Breaking if anchor.major > 0 => VersionBump::Major,
            Self::Breaking if anchor.minor > 0 => VersionBump::Minor,
            Self::Nonbreaking if anchor.major > 0 => VersionBump::Minor,
            Self::Breaking | Self::Nonbreaking | Self::Patch => VersionBump::Patch,
        };
        increment_version(anchor, bump)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Breaking => "breaking",
            Self::Nonbreaking => "nonbreaking",
            Self::Patch => "patch",
        }
    }
}

impl Proposal<'_> {
    pub(crate) fn decision_increments(
        &self,
        impacts: &BTreeMap<String, SemanticImpact>,
        verbose: &impl NoteSink,
    ) -> Result<Vec<PlanIncrement>, AppError> {
        let mut increments = Vec::new();
        for (name, impact) in impacts {
            if !self.packages.contains_key(name.as_str()) {
                return Err(UnknownDecisionTarget::new(name).into());
            }
            let anchor = self
                .anchors
                .get(name.as_str())
                .ok_or_else(|| FirstPublicationRequired::new(name))?;
            let declared = self.declared(name);
            // Component bumps cannot express dropping a prerelease suffix, so deriving a
            // mechanical bump from a prerelease would silently overshoot its semantic target.
            if !anchor.pre.is_empty() || !declared.pre.is_empty() {
                return Err(PrereleaseDecision::new(name).into());
            }
            let minimum = impact.minimum(anchor)?;
            if declared.cmp_precedence(&minimum).is_ge() {
                verbose.note(|| {
                    format!(
                        "Decision for package {} at semantic impact '{}' is not emitted because \
                         declared version {} already satisfies minimum version {} from anchor {}.",
                        quote_path(name),
                        impact.name(),
                        declared,
                        minimum,
                        anchor
                    )
                });
                continue;
            }
            let bump = if minimum.major > declared.major {
                VersionBump::Major
            } else if minimum.minor > declared.minor {
                VersionBump::Minor
            } else {
                VersionBump::Patch
            };
            verbose.note(|| {
                format!(
                    "Decision for package {} at semantic impact '{}' is emitted as '{}' because \
                     declared version {} is below minimum version {} from anchor {}.",
                    quote_path(name),
                    impact.name(),
                    bump,
                    declared,
                    minimum,
                    anchor
                )
            });
            increments.push(PlanIncrement {
                name: name.clone(),
                bump: Some(bump.to_string()),
                version: None,
            });
        }
        Ok(increments)
    }
}

/// A decision document must preserve the skill's typed name/impact contract.
#[ohno::error]
#[display("invalid release change-decision document")]
struct InvalidDecisionFile;

/// The decision working-file protocol is versioned independently from the report protocol.
#[ohno::error]
#[display("unsupported change-decision schema_version {version}; expected 2")]
struct UnsupportedDecisionSchema {
    version: u32,
}

/// Names are nonempty, unique ordinal identifiers rather than normalized labels.
#[ohno::error]
#[display("change-decision name {} is empty or duplicated", name.quoted())]
struct InvalidDecisionName {
    name: String,
}

/// Only publishable packages can receive semantic assessments.
#[ohno::error]
#[display("change decision names an unknown or non-publishable package {}", name.quoted())]
struct UnknownDecisionTarget {
    name: String,
}

/// First publication requires the release process rather than an inferred anchor.
#[ohno::error]
#[display("package {} has no release anchor; omit it from increment decisions and handle its initial publication through your release process", name.quoted())]
struct FirstPublicationRequired {
    name: String,
}

/// Semantic component-based assessment requires release versions at both endpoints.
#[ohno::error]
#[display("package {} has a prerelease version, which semantic proposal generation does not support", name.quoted())]
struct PrereleaseDecision {
    name: String,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::cell::RefCell;

    use serde_json::json;

    use super::*;
    use crate::VersionOverflowError;
    use crate::propose::tests::{generate, helper, package, report};

    #[test]
    fn verbose_decisions_explain_emitted_and_retained_versions() {
        let report = report(
            vec![
                package("breaking", "1.0.0", Some("1.0.0")),
                package("feature", "1.0.0", Some("1.0.0")),
                package("patch", "1.0.0", Some("1.0.0")),
                package("pending-breaking", "2.0.0", Some("1.0.0")),
                package("pending-feature", "1.1.0", Some("1.0.0")),
                package("pending-patch", "1.0.1", Some("1.0.0")),
            ],
            vec![],
            &[],
        );
        report.validate().unwrap();
        let impacts = BTreeMap::from([
            ("breaking".to_owned(), SemanticImpact::Breaking),
            ("feature".to_owned(), SemanticImpact::Nonbreaking),
            ("patch".to_owned(), SemanticImpact::Patch),
            ("pending-breaking".to_owned(), SemanticImpact::Breaking),
            ("pending-feature".to_owned(), SemanticImpact::Nonbreaking),
            ("pending-patch".to_owned(), SemanticImpact::Patch),
        ]);
        let notes = RefCell::new(Vec::new());
        let increments = Proposal::new(&report)
            .decision_increments(&impacts, &notes)
            .unwrap();
        assert_eq!(
            increments,
            [
                PlanIncrement {
                    name: "breaking".to_owned(),
                    bump: Some("major".to_owned()),
                    version: None,
                },
                PlanIncrement {
                    name: "feature".to_owned(),
                    bump: Some("minor".to_owned()),
                    version: None,
                },
                PlanIncrement {
                    name: "patch".to_owned(),
                    bump: Some("patch".to_owned()),
                    version: None,
                },
            ]
        );
        let notes = notes.into_inner();
        assert_eq!(notes.len(), impacts.len());
        for (name, semantic_impact, declared, minimum, emitted) in [
            ("breaking", "breaking", "1.0.0", "2.0.0", true),
            ("feature", "nonbreaking", "1.0.0", "1.1.0", true),
            ("patch", "patch", "1.0.0", "1.0.1", true),
            ("pending-breaking", "breaking", "2.0.0", "2.0.0", false),
            ("pending-feature", "nonbreaking", "1.1.0", "1.1.0", false),
            ("pending-patch", "patch", "1.0.1", "1.0.1", false),
        ] {
            let note = notes
                .iter()
                .find(|note| note.contains(&format!("package {}", quote_path(name))))
                .unwrap();
            assert!(note.contains(&format!("semantic impact '{semantic_impact}'")));
            assert!(note.contains(&format!("declared version {declared}")));
            assert!(note.contains(&format!("minimum version {minimum}")));
            assert!(note.contains("anchor 1.0.0"));
            assert_eq!(note.contains("not emitted"), !emitted);
        }
    }

    #[test]
    fn malformed_decision_shapes_fail_typed_parsing() {
        for value in [
            json!(null),
            json!([]),
            json!({"changes": []}),
            json!({"schema_version": "1", "changes": []}),
            json!({"schema_version": 2.0, "changes": []}),
            json!({"schema_version": 2}),
            json!({"schema_version": 2, "changes": {}}),
            json!({"schema_version": 2, "changes": [null]}),
            json!({"schema_version": 2, "changes": [{"name": "lib"}]}),
            json!({"schema_version": 2, "changes": [{"name": "lib", "impact": "patch", "version": "9.0.0"}]}),
            json!({"schema_version": 2, "changes": [{"name": "lib", "impact": "minor"}]}),
            json!({"schema_version": 2, "changes": [{"name": "lib", "impact": "Breaking"}]}),
            json!({"schema_version": 2, "changes": [{"name": "lib", "impact": null}]}),
        ] {
            let error = Decisions::parse(&value.to_string()).unwrap_err();
            assert!(error.find_source::<InvalidDecisionFile>().is_some());
        }
        let error = Decisions::parse(
            r#"{"schema_version":2,"changes":[{"name":"lib","name":"other","impact":"patch"}]}"#,
        )
        .unwrap_err();
        assert!(error.find_source::<InvalidDecisionFile>().is_some());
    }

    #[test]
    fn schema_and_ordinal_unique_names_are_validated() {
        for schema_version in [DECISION_SCHEMA_VERSION - 1, DECISION_SCHEMA_VERSION + 1] {
            let decisions = Decisions::parse(
                &json!({"schema_version": schema_version, "changes": []}).to_string(),
            )
            .unwrap();
            let error = decisions.validate().unwrap_err();
            assert!(error.find_source::<UnsupportedDecisionSchema>().is_some());
        }
        for names in [["lib", "lib"], [" ", "lib"]] {
            let decisions = Decisions::parse(
                &json!({
                    "schema_version": 2,
                    "changes": names.map(|name| json!({"name": name, "impact": "patch"}))
                })
                .to_string(),
            )
            .unwrap();
            let error = decisions.validate().unwrap_err();
            assert!(error.find_source::<InvalidDecisionName>().is_some());
        }
        let decisions = Decisions::parse(
            r#"{"schema_version":2,"metadata":"allowed","changes":[{"name":"lib","impact":"patch"},{"name":"Lib","impact":"patch"}]}"#,
        )
        .unwrap();
        assert_eq!(decisions.validate().unwrap().len(), 2);
        assert!(
            Decisions::parse("\u{feff}{\"schema_version\":2,\"changes\":[]}")
                .unwrap()
                .validate()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn semantic_decisions_reject_mechanical_bumps_and_retired_field_names() {
        for change in [
            json!({"name": "lib", "level": "patch"}),
            json!({"name": "lib", "impact": "patch", "level": "patch"}),
            json!({"name": "lib", "bump": "patch"}),
            json!({"name": "lib", "impact": "major"}),
        ] {
            let document = json!({
                "schema_version": DECISION_SCHEMA_VERSION,
                "changes": [change],
            });
            let error = Decisions::parse(&document.to_string()).unwrap_err();
            assert!(error.find_source::<InvalidDecisionFile>().is_some());
        }
    }

    #[test]
    fn semantic_algebra_covers_stable_zero_minor_and_zero_patch_lines() {
        for (anchor, expected) in [
            ("2.4.7", ["3.0.0", "2.5.0", "2.4.8"]),
            ("0.7.14", ["0.8.0", "0.7.15", "0.7.15"]),
            ("0.0.5", ["0.0.6", "0.0.6", "0.0.6"]),
        ] {
            let anchor = anchor.parse::<Version>().unwrap();
            for (impact, expected) in [
                SemanticImpact::Breaking,
                SemanticImpact::Nonbreaking,
                SemanticImpact::Patch,
            ]
            .into_iter()
            .zip(expected)
            {
                assert_eq!(impact.minimum(&anchor).unwrap().to_string(), expected);
            }
        }
        let error = SemanticImpact::Patch
            .minimum(&Version::new(1, 0, u64::MAX))
            .unwrap_err();
        assert!(error.find_source::<VersionOverflowError>().is_some());
    }

    #[test]
    fn invalid_targets_and_first_publications_are_not_semantic_decisions() {
        let report = report(
            vec![package("new", "0.1.0", None)],
            vec![helper("helper", "1.0.0")],
            &[],
        );
        for target in ["absent", "helper"] {
            let error = generate(&report, &[(target, "patch")]).unwrap_err();
            assert!(error.find_source::<UnknownDecisionTarget>().is_some());
        }
        let error = generate(&report, &[("new", "patch")]).unwrap_err();
        assert!(error.find_source::<FirstPublicationRequired>().is_some());
    }

    #[test]
    fn prerelease_semantic_endpoints_are_rejected_even_when_a_group_covers_them() {
        for (declared, anchor) in [("1.1.0-alpha", "1.0.0"), ("1.1.0", "1.0.0-alpha")] {
            let report = report(
                vec![
                    package("lib", declared, Some(anchor)),
                    package("sibling", "2.0.0", Some("2.0.0")),
                ],
                vec![],
                &[&["lib", "sibling"]],
            );
            let error = generate(&report, &[("lib", "patch")]).unwrap_err();
            assert!(error.find_source::<PrereleaseDecision>().is_some());
        }
    }

    #[test]
    fn adequate_pending_breaking_increment_is_kept() {
        assert_pending_increment("0.8.0", "0.7.0", "breaking", None);
    }

    #[test]
    fn adequate_pending_patch_increment_is_kept() {
        assert_pending_increment("1.1.0", "1.0.0", "patch", None);
    }

    #[test]
    fn inadequate_pending_nonbreaking_increment_is_raised() {
        assert_pending_increment("1.0.1", "1.0.0", "nonbreaking", Some("minor"));
    }

    #[test]
    fn inadequate_pending_breaking_increment_is_raised() {
        assert_pending_increment("1.1.0", "1.0.0", "breaking", Some("major"));
    }

    #[test]
    fn build_metadata_does_not_satisfy_a_pending_patch_increment() {
        assert_pending_increment("1.0.0+build", "1.0.0", "patch", Some("patch"));
    }

    fn assert_pending_increment(
        declared: &str,
        anchor: &str,
        change: &str,
        expected: Option<&str>,
    ) {
        let report = report(vec![package("lib", declared, Some(anchor))], vec![], &[]);
        let plan = generate(&report, &[("lib", change)]).unwrap();
        assert_eq!(
            plan.increments
                .first()
                .and_then(|entry| entry.bump.as_deref()),
            expected
        );
    }
}
