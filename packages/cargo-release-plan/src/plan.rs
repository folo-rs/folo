// Increment-plan parsing and group expansion.
//
// The new version for a group is the highest version declared by any member,
// raised by the highest required level. An explicit `version` overrides that
// arithmetic for the named target and its group.

use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;

use ohno::AppError;
use semver::Version;
use serde::Deserialize;

use crate::groups::Groups;
use crate::{
    ConflictingPlanVersionError, InvalidVersionError, PlanIncrementSpecError,
    PlanVersionRegressionError, UnknownIncrementLevelError, UnknownPlanTargetError,
    UnsupportedPlanSchemaError, VersionOverflowError,
};

/// Shared plan and report schema revision.
///
/// Plan and report formats advance together. Incompatible field, enum, or
/// path-layout changes increment this constant. Contract: package README
/// "Plan and report schema".
pub(crate) const SCHEMA_VERSION: u32 = 1;

/// Requested version increment relative to the highest declared member version.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) enum IncrementLevel {
    Patch,
    Minor,
    Major,
}

impl FromStr for IncrementLevel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "patch" => Ok(Self::Patch),
            "minor" => Ok(Self::Minor),
            "major" => Ok(Self::Major),
            _ => Err(()),
        }
    }
}

/// One increment entry as stored in plan JSON.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct PlanIncrement {
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) level: Option<String>,
    #[serde(default)]
    pub(crate) version: Option<String>,
}

impl PlanIncrement {
    fn spec(&self) -> Result<IncrementSpec, AppError> {
        match (&self.level, &self.version) {
            (Some(level), None) => {
                let level = IncrementLevel::from_str(level)
                    .map_err(|()| UnknownIncrementLevelError::new(&self.name, level))?;
                Ok(IncrementSpec::Level(level))
            }
            (None, Some(version)) => {
                let parsed = version
                    .parse::<Version>()
                    .map_err(|error| InvalidVersionError::caused_by(&self.name, version, error))?;
                Ok(IncrementSpec::Version(parsed))
            }
            (None, None) | (Some(_), Some(_)) => {
                Err(PlanIncrementSpecError::new(&self.name).into())
            }
        }
    }
}

/// Parsed form of exactly one of `level` or `version`.
enum IncrementSpec {
    Level(IncrementLevel),
    Version(Version),
}

/// On-disk plan file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct PlanFile {
    pub(crate) schema_version: u32,
    pub(crate) increments: Vec<PlanIncrement>,
}

/// Package name → new version after group expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpandedPlan {
    pub(crate) packages: BTreeMap<String, Version>,
}

/// Accumulated decision for one group or ungrouped package.
struct Decision {
    level: Option<IncrementLevel>,
    version: Option<Version>,
}

/// Expands `plan` against work-tree versions and group membership.
///
/// `publishable` is the set of packages `apply` is allowed to touch.
pub(crate) fn expand_plan(
    plan: &PlanFile,
    groups: &Groups,
    current: &BTreeMap<String, Version>,
    publishable: &BTreeMap<String, Version>,
) -> Result<ExpandedPlan, AppError> {
    if plan.schema_version != SCHEMA_VERSION {
        return Err(UnsupportedPlanSchemaError::new(plan.schema_version).into());
    }

    let mut decisions: BTreeMap<String, Decision> = BTreeMap::new();

    for increment in &plan.increments {
        let targets = resolve_targets(&increment.name, groups, publishable)?;
        let keys = decision_keys(&targets, groups);
        match increment.spec()? {
            IncrementSpec::Level(level) => {
                for key in keys {
                    let decision = decisions.entry(key).or_insert(Decision {
                        level: None,
                        version: None,
                    });
                    decision.level =
                        Some(decision.level.map_or(level, |existing| existing.max(level)));
                }
            }
            IncrementSpec::Version(version) => {
                for key in keys {
                    let decision = decisions.entry(key.clone()).or_insert(Decision {
                        level: None,
                        version: None,
                    });
                    if let Some(existing) = &decision.version
                        && existing != &version
                    {
                        return Err(ConflictingPlanVersionError::new(key).into());
                    }
                    decision.version = Some(version.clone());
                }
            }
        }
    }

    let mut packages = BTreeMap::new();
    for (key, decision) in decisions {
        let members = members_for_key(&key, groups, publishable);
        let highest = members
            .iter()
            .filter_map(|member| current.get(member))
            .max()
            .cloned();
        let new_version = if let Some(version) = decision.version {
            // An explicit version still has to move forwards: a lower one would
            // re-publish a released version with different content.
            // Ref: docs/design.md, "Version monotonicity".
            if let Some(highest) = &highest
                && &version < highest
            {
                return Err(PlanVersionRegressionError::new(&key, version, highest.clone()).into());
            }
            version
        } else if let Some(level) = decision.level {
            let Some(highest) = highest else {
                continue;
            };
            increment_version(&highest, level)?
        } else {
            continue;
        };
        for member in members {
            packages.insert(member, new_version.clone());
        }
    }

    Ok(ExpandedPlan { packages })
}

fn resolve_targets(
    name: &str,
    groups: &Groups,
    publishable: &BTreeMap<String, Version>,
) -> Result<Vec<String>, AppError> {
    let group_members = groups.members(name);
    if !group_members.is_empty() {
        let targets: Vec<String> = group_members
            .iter()
            .filter(|member| publishable.contains_key(*member))
            .cloned()
            .collect();
        if targets.is_empty() {
            // Silently dropping the entry would report success while applying
            // nothing, which reads as an accepted plan that had no effect.
            return Err(UnknownPlanTargetError::new(name).into());
        }
        return Ok(targets);
    }
    if publishable.contains_key(name) {
        return Ok(groups
            .closure(name)
            .into_iter()
            .filter(|member| publishable.contains_key(member))
            .collect());
    }
    Err(UnknownPlanTargetError::new(name).into())
}

fn decision_keys(targets: &[String], groups: &Groups) -> BTreeSet<String> {
    targets
        .iter()
        .map(|target| groups.group_of(target).unwrap_or(target).to_string())
        .collect()
}

fn members_for_key(
    key: &str,
    groups: &Groups,
    publishable: &BTreeMap<String, Version>,
) -> Vec<String> {
    let group_members = groups.members(key);
    if group_members.is_empty() {
        if publishable.contains_key(key) {
            vec![key.to_string()]
        } else {
            Vec::new()
        }
    } else {
        group_members
            .iter()
            .filter(|member| publishable.contains_key(*member))
            .cloned()
            .collect()
    }
}

pub(crate) fn increment_version(
    version: &Version,
    level: IncrementLevel,
) -> Result<Version, AppError> {
    match level {
        IncrementLevel::Major => {
            let major = version
                .major
                .checked_add(1)
                .ok_or_else(|| VersionOverflowError::new(version.clone()))?;
            Ok(Version::new(major, 0, 0))
        }
        IncrementLevel::Minor => {
            let minor = version
                .minor
                .checked_add(1)
                .ok_or_else(|| VersionOverflowError::new(version.clone()))?;
            Ok(Version::new(version.major, minor, 0))
        }
        IncrementLevel::Patch => {
            let patch = version
                .patch
                .checked_add(1)
                .ok_or_else(|| VersionOverflowError::new(version.clone()))?;
            Ok(Version::new(version.major, version.minor, patch))
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn nm_groups() -> Groups {
        Groups::from_members(BTreeMap::from([(
            "nm".to_string(),
            vec!["nm".to_string(), "nm_impl".to_string()],
        )]))
        .unwrap()
    }

    fn current() -> BTreeMap<String, Version> {
        // Synthetic versions; tests assert relative increment arithmetic, not
        // workspace pins.
        BTreeMap::from([
            ("nm".to_string(), v("0.1.0")),
            ("nm_impl".to_string(), v("0.1.0")),
            ("events".to_string(), v("0.2.0")),
        ])
    }

    #[test]
    fn only_the_three_semver_levels_are_accepted() {
        assert_eq!("patch".parse::<IncrementLevel>(), Ok(IncrementLevel::Patch));
        assert_eq!("minor".parse::<IncrementLevel>(), Ok(IncrementLevel::Minor));
        assert_eq!("major".parse::<IncrementLevel>(), Ok(IncrementLevel::Major));
        "Patch".parse::<IncrementLevel>().unwrap_err();
        "build".parse::<IncrementLevel>().unwrap_err();
    }

    /// A level has to be applied to some declared version. A group whose members
    /// have never been published has none, so there is nothing to increment.
    #[test]
    fn a_level_on_a_group_with_no_declared_version_expands_to_nothing() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };

        let expanded = expand_plan(&plan, &nm_groups(), &BTreeMap::new(), &current()).unwrap();

        assert!(expanded.packages.is_empty());
    }

    #[test]
    fn expands_group_when_one_member_is_listed() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm_impl".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("nm"), Some(&v("0.1.1")));
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.1.1")));
        assert!(!expanded.packages.contains_key("events"));
    }

    #[test]
    fn highest_level_wins_inside_a_group() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![
                PlanIncrement {
                    name: "nm".to_string(),
                    level: Some("patch".to_string()),
                    version: None,
                },
                PlanIncrement {
                    name: "nm_impl".to_string(),
                    level: Some("minor".to_string()),
                    version: None,
                },
            ],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("nm"), Some(&v("0.2.0")));
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.2.0")));
    }

    #[test]
    fn explicit_version_is_applied_to_the_group() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm".to_string(),
                level: None,
                version: Some("0.2.0".to_string()),
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("nm"), Some(&v("0.2.0")));
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.2.0")));
    }

    #[test]
    fn rejects_unknown_schema() {
        let plan = PlanFile {
            // Arbitrary revision distinct from the supported schema.
            schema_version: 9,
            increments: vec![],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }

    #[test]
    fn rejects_unknown_target() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "ghost".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        assert!(error.find_source::<UnknownPlanTargetError>().is_some());
    }

    #[test]
    fn rejects_missing_level_and_version() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "events".to_string(),
                level: None,
                version: None,
            }],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        assert!(error.find_source::<PlanIncrementSpecError>().is_some());
    }

    #[test]
    fn max_declared_version_is_the_increment_base() {
        let mut versions = current();
        versions.insert("nm_impl".to_string(), v("0.1.50"));
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &versions, &versions).unwrap();
        assert_eq!(expanded.packages.get("nm"), Some(&v("0.1.51")));
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.1.51")));
    }

    #[test]
    fn rejects_conflicting_explicit_versions() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![
                PlanIncrement {
                    name: "nm".to_string(),
                    level: None,
                    version: Some("0.2.0".to_string()),
                },
                PlanIncrement {
                    name: "nm_impl".to_string(),
                    level: None,
                    version: Some("0.3.0".to_string()),
                },
            ],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        assert!(error.find_source::<ConflictingPlanVersionError>().is_some());
    }

    #[test]
    fn ungrouped_package_is_incremented_alone() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "events".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("events"), Some(&v("0.2.1")));
        assert!(!expanded.packages.contains_key("nm"));
    }

    #[test]
    fn major_level_increments_the_major_component() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "events".to_string(),
                level: Some("major".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("events"), Some(&v("1.0.0")));
    }

    #[test]
    fn increment_version_errors_when_a_component_overflows() {
        let max = Version::new(0, 0, u64::MAX);
        let error = increment_version(&max, IncrementLevel::Patch).unwrap_err();
        assert!(error.find_source::<VersionOverflowError>().is_some());
    }

    #[test]
    fn explicit_version_below_the_declared_version_is_rejected() {
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "events".to_string(),
                level: None,
                version: Some("0.1.0".to_string()),
            }],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        let regression = error.find_source::<PlanVersionRegressionError>().unwrap();
        assert_eq!(regression.target(), "events");
    }

    #[test]
    fn explicit_version_equal_to_the_declared_version_is_accepted() {
        // Equality is how a lagging group member is raised into alignment.
        let mut current = current();
        current.insert("nm_impl".to_string(), v("0.0.9"));
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm".to_string(),
                level: None,
                version: Some("0.1.0".to_string()),
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current, &current).unwrap();
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.1.0")));
    }

    #[test]
    fn group_without_publishable_members_is_rejected() {
        let publishable = BTreeMap::from([("events".to_string(), v("0.2.0"))]);
        let plan = PlanFile {
            schema_version: SCHEMA_VERSION,
            increments: vec![PlanIncrement {
                name: "nm".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &publishable).unwrap_err();
        assert!(error.find_source::<UnknownPlanTargetError>().is_some());
    }
}
