// Increment-plan parsing and group expansion.
//
// The new version for a group is the highest version declared by any member,
// raised by the highest required level. An explicit `version` overrides that
// arithmetic for the named target and its group.

use std::collections::BTreeMap;
use std::str::FromStr;

use ohno::AppError;
use semver::Version;
use serde::Deserialize;

use crate::groups::Groups;
use crate::{
    ConflictingPlanVersionError, InvalidVersionError, PlanIncrementSpecError, SCHEMA_VERSION,
    UnknownIncrementLevelError, UnknownPlanTargetError, UnsupportedPlanSchemaError,
    VersionOverflowError,
};

/// Requested bump relative to the highest declared member version.
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
    pub name: String,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// On-disk plan file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
pub(crate) struct PlanFile {
    pub schema_version: u32,
    pub increments: Vec<PlanIncrement>,
}

/// Package name → new version after group expansion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ExpandedPlan {
    pub packages: BTreeMap<String, Version>,
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
        match (&increment.level, &increment.version) {
            (Some(level), None) => {
                let parsed = IncrementLevel::from_str(level)
                    .map_err(|()| UnknownIncrementLevelError::new(&increment.name, level))?;
                for key in keys {
                    let decision = decisions.entry(key).or_insert(Decision {
                        level: None,
                        version: None,
                    });
                    decision.level = Some(
                        decision
                            .level
                            .map_or(parsed, |existing| existing.max(parsed)),
                    );
                }
            }
            (None, Some(version)) => {
                let parsed = version.parse::<Version>().map_err(|error| {
                    InvalidVersionError::caused_by(&increment.name, version, error)
                })?;
                for key in keys {
                    let decision = decisions.entry(key.clone()).or_insert(Decision {
                        level: None,
                        version: None,
                    });
                    if let Some(existing) = &decision.version
                        && existing != &parsed
                    {
                        return Err(ConflictingPlanVersionError::new(key).into());
                    }
                    decision.version = Some(parsed.clone());
                }
            }
            (None, None) | (Some(_), Some(_)) => {
                return Err(PlanIncrementSpecError::new(&increment.name).into());
            }
        }
    }

    let mut packages = BTreeMap::new();
    for (key, decision) in decisions {
        let members = members_for_key(&key, groups, publishable);
        let new_version = if let Some(version) = decision.version {
            version
        } else if let Some(level) = decision.level {
            let Some(max) = members
                .iter()
                .filter_map(|member| current.get(member))
                .max()
                .cloned()
            else {
                continue;
            };
            bump(&max, level)?
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
        return Ok(group_members
            .iter()
            .filter(|member| publishable.contains_key(*member))
            .cloned()
            .collect());
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

fn decision_keys(targets: &[String], groups: &Groups) -> Vec<String> {
    let mut keys = Vec::new();
    for target in targets {
        let key = groups.group_of(target).unwrap_or(target).to_string();
        if !keys.contains(&key) {
            keys.push(key);
        }
    }
    keys
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

pub(crate) fn bump(version: &Version, level: IncrementLevel) -> Result<Version, AppError> {
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
        BTreeMap::from([
            ("nm".to_string(), v("0.1.43")),
            ("nm_impl".to_string(), v("0.1.43")),
            ("events".to_string(), v("0.7.13")),
        ])
    }

    #[test]
    fn expands_group_when_one_member_is_listed() {
        let plan = PlanFile {
            schema_version: 1,
            increments: vec![PlanIncrement {
                name: "nm_impl".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("nm"), Some(&v("0.1.44")));
        assert_eq!(expanded.packages.get("nm_impl"), Some(&v("0.1.44")));
        assert!(!expanded.packages.contains_key("events"));
    }

    #[test]
    fn highest_level_wins_inside_a_group() {
        let plan = PlanFile {
            schema_version: 1,
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
            schema_version: 1,
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
            schema_version: 9,
            increments: vec![],
        };
        let error = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }

    #[test]
    fn rejects_unknown_target() {
        let plan = PlanFile {
            schema_version: 1,
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
            schema_version: 1,
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
    fn max_declared_version_is_the_bump_base() {
        let mut versions = current();
        versions.insert("nm_impl".to_string(), v("0.1.50"));
        let plan = PlanFile {
            schema_version: 1,
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
            schema_version: 1,
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
    fn ungrouped_package_is_bumped_alone() {
        let plan = PlanFile {
            schema_version: 1,
            increments: vec![PlanIncrement {
                name: "events".to_string(),
                level: Some("patch".to_string()),
                version: None,
            }],
        };
        let expanded = expand_plan(&plan, &nm_groups(), &current(), &current()).unwrap();
        assert_eq!(expanded.packages.get("events"), Some(&v("0.7.14")));
        assert!(!expanded.packages.contains_key("nm"));
    }

    #[test]
    fn major_level_bumps_the_major_component() {
        let plan = PlanFile {
            schema_version: 1,
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
    fn bump_errors_when_a_component_overflows() {
        let max = Version::new(0, 0, u64::MAX);
        let error = bump(&max, IncrementLevel::Patch).unwrap_err();
        assert!(error.find_source::<VersionOverflowError>().is_some());
    }
}
