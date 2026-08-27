// Version-group membership and consistency.
//
// Groups are declared in `[workspace.metadata.release-plan.groups]`. Members
// share a declared version; members absent from the base revision are exempt
// from that consistency rule so a new crate can join a group before it is
// published.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use ohno::AppError;
use semver::Version;

use crate::DuplicateGroupMemberError;

/// Version groups keyed by group name and by package name.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Groups {
    by_name: BTreeMap<String, Vec<String>>,
    by_package: BTreeMap<String, String>,
}

impl Groups {
    pub(crate) fn from_members(map: BTreeMap<String, Vec<String>>) -> Result<Self, AppError> {
        let mut by_package = BTreeMap::new();
        for (group, members) in &map {
            let mut seen_in_group = HashSet::new();
            for member in members {
                if !seen_in_group.insert(member) {
                    return Err(DuplicateGroupMemberError::new(member, group, group).into());
                }
                if let Some(first) = by_package.get(member) {
                    return Err(DuplicateGroupMemberError::new(member, first, group).into());
                }
                by_package.insert(member.clone(), group.clone());
            }
        }
        Ok(Self {
            by_name: map,
            by_package,
        })
    }

    pub(crate) fn group_of(&self, package: &str) -> Option<&str> {
        self.by_package.get(package).map(String::as_str)
    }

    pub(crate) fn members(&self, group: &str) -> &[String] {
        self.by_name.get(group).map_or(&[], Vec::as_slice)
    }

    /// Packages that share a group with `package`, including `package` itself.
    pub(crate) fn closure(&self, package: &str) -> Vec<String> {
        match self.group_of(package) {
            Some(group) => self.members(group).to_vec(),
            None => vec![package.to_string()],
        }
    }

    /// Group-level consistency on work-tree declared versions.
    ///
    /// `exempt` names members that do not exist on the base revision and are
    /// therefore not required to match.
    pub(crate) fn verdicts(
        &self,
        versions: &BTreeMap<String, Version>,
        exempt: &HashSet<String>,
    ) -> BTreeMap<String, GroupVerdict> {
        let mut out = BTreeMap::new();
        for (name, members) in &self.by_name {
            let present: Vec<&str> = members
                .iter()
                .map(String::as_str)
                .filter(|member| versions.contains_key(*member))
                .collect();
            let compared: Vec<&str> = present
                .iter()
                .copied()
                .filter(|member| !exempt.contains(*member))
                .collect();
            let compared_versions: BTreeSet<&Version> = compared
                .iter()
                .filter_map(|member| versions.get(*member))
                .collect();
            let consistent = compared_versions.len() <= 1;
            let version = compared_versions
                .iter()
                .next()
                .copied()
                .cloned()
                .or_else(|| {
                    present
                        .iter()
                        .filter_map(|member| versions.get(*member))
                        .max()
                        .cloned()
                });
            out.insert(
                name.clone(),
                GroupVerdict {
                    members: present.into_iter().map(ToOwned::to_owned).collect(),
                    consistent,
                    version,
                },
            );
        }
        out
    }
}

/// Consistency outcome for one version group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GroupVerdict {
    pub(crate) members: Vec<String>,
    pub(crate) consistent: bool,
    pub(crate) version: Option<Version>,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        text.parse().unwrap()
    }

    fn groups() -> Groups {
        Groups::from_members(BTreeMap::from([(
            "nm".to_string(),
            vec!["nm".to_string(), "nm_impl".to_string()],
        )]))
        .unwrap()
    }

    #[test]
    fn duplicate_member_across_groups_is_rejected() {
        let error = Groups::from_members(BTreeMap::from([
            ("a".to_string(), vec!["shared".to_string()]),
            ("b".to_string(), vec!["shared".to_string()]),
        ]))
        .unwrap_err();
        assert!(error.find_source::<DuplicateGroupMemberError>().is_some());
    }

    #[test]
    fn duplicate_member_inside_one_group_is_rejected() {
        let error = Groups::from_members(BTreeMap::from([(
            "nm".to_string(),
            vec!["nm".to_string(), "nm".to_string()],
        )]))
        .unwrap_err();
        assert!(error.find_source::<DuplicateGroupMemberError>().is_some());
    }

    #[test]
    fn consistent_when_declared_versions_match() {
        let versions = BTreeMap::from([
            ("nm".to_string(), v("0.1.0")),
            ("nm_impl".to_string(), v("0.1.0")),
        ]);
        let verdicts = groups().verdicts(&versions, &HashSet::new());
        let nm = verdicts.get("nm").unwrap();
        assert!(nm.consistent);
        assert_eq!(nm.version.as_ref().unwrap(), &v("0.1.0"));
    }

    #[test]
    fn inconsistent_when_declared_versions_differ() {
        let versions = BTreeMap::from([
            ("nm".to_string(), v("0.1.0")),
            ("nm_impl".to_string(), v("0.1.1")),
        ]);
        let verdicts = groups().verdicts(&versions, &HashSet::new());
        assert!(!verdicts.get("nm").unwrap().consistent);
    }

    #[test]
    fn never_published_member_is_exempt_from_consistency() {
        let versions = BTreeMap::from([
            ("nm".to_string(), v("0.2.0")),
            ("nm_impl".to_string(), v("0.1.0")),
        ]);
        let exempt = HashSet::from(["nm_impl".to_string()]);
        let verdicts = groups().verdicts(&versions, &exempt);
        let nm = verdicts.get("nm").unwrap();
        assert!(nm.consistent);
        assert_eq!(nm.version.as_ref().unwrap(), &v("0.2.0"));
    }

    #[test]
    fn closure_includes_every_member() {
        assert_eq!(groups().closure("nm_impl"), vec!["nm", "nm_impl"]);
        let empty = Groups::default();
        assert_eq!(empty.closure("events"), vec!["events"]);
    }
}
