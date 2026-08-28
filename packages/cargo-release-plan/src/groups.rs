// Version-group membership and consistency.
//
// Groups are declared in `[workspace.metadata.release-plan.groups]`. Members
// share a declared version; members absent from the base revision are exempt
// from that consistency rule so a new package can join a group before it is
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
        self.by_name
            .iter()
            .map(|(name, members)| (name.clone(), GroupVerdict::new(members, versions, exempt)))
            .collect()
    }
}

/// Consistency outcome for one version group.
///
/// The outcome is derived once, at construction, from the declared versions and
/// the exemption set; there is no way to assemble a verdict that contradicts
/// those facts. That matters because `check` gates the process exit on
/// consistency while `report` and `apply` use the group version as the
/// increment base, so a verdict that reported one without the other would let
/// the two disagree. Ref: `docs/design.md`, "Version groups".
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct GroupVerdict {
    members: Vec<String>,
    state: GroupState,
}

impl GroupVerdict {
    /// Derives the verdict for one group from the work tree's declared versions.
    ///
    /// `members` is the group as declared in the manifest; only members that
    /// have a declared version participate. `exempt` names members that do not
    /// exist on the base revision.
    pub(crate) fn new(
        members: &[String],
        versions: &BTreeMap<String, Version>,
        exempt: &HashSet<String>,
    ) -> Self {
        let members: Vec<String> = members
            .iter()
            .filter(|member| versions.contains_key(*member))
            .cloned()
            .collect();
        let compared: BTreeSet<&Version> = members
            .iter()
            .filter(|member| !exempt.contains(*member))
            .filter_map(|member| versions.get(member))
            .collect();
        // Exemption governs consistency only. The group version is the highest
        // declared by any present member, including exempt ones, so that it
        // matches the increment base `expand_plan` computes and no member is
        // ever moved backwards.
        let highest = members
            .iter()
            .filter_map(|member| versions.get(member))
            .max()
            .cloned();
        let state = match highest {
            None => GroupState::Empty,
            Some(version) if compared.len() <= 1 => GroupState::Consistent { version },
            Some(version) => GroupState::Inconsistent { version },
        };
        Self { members, state }
    }

    /// Members that declare a version, in manifest order.
    pub(crate) fn members(&self) -> &[String] {
        &self.members
    }

    /// Whether every non-exempt member declares the same version.
    pub(crate) fn is_consistent(&self) -> bool {
        !matches!(self.state, GroupState::Inconsistent { .. })
    }

    /// The highest version any member declares, absent only for an empty group.
    pub(crate) fn version(&self) -> Option<&Version> {
        match &self.state {
            GroupState::Consistent { version } | GroupState::Inconsistent { version } => {
                Some(version)
            }
            GroupState::Empty => None,
        }
    }
}

/// The outcomes a group can actually have.
///
/// A group with no participating member has no version to report; every other
/// group has one, whether or not its members agree. Keeping that as a closed set
/// of alternatives — rather than a flag beside an optional version — leaves no
/// way to express an inconsistency without the baseline version that planning
/// needs.
#[derive(Clone, Debug, Eq, PartialEq)]
enum GroupState {
    /// No member of the group declares a version.
    Empty,
    /// Every non-exempt member declares the same version.
    Consistent { version: Version },
    /// Non-exempt members declare more than one version.
    Inconsistent { version: Version },
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
        assert!(nm.is_consistent());
        assert_eq!(nm.version().unwrap(), &v("0.1.0"));
    }

    #[test]
    fn inconsistent_when_declared_versions_differ() {
        let versions = BTreeMap::from([
            ("nm".to_string(), v("0.1.0")),
            ("nm_impl".to_string(), v("0.1.1")),
        ]);
        let verdicts = groups().verdicts(&versions, &HashSet::new());
        assert!(!verdicts.get("nm").unwrap().is_consistent());
        // The reported version is the highest declared by any present member,
        // so it can serve as the increment base for the whole group.
        assert_eq!(verdicts.get("nm").unwrap().version().unwrap(), &v("0.1.1"));
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
        assert!(nm.is_consistent());
        assert_eq!(nm.version().unwrap(), &v("0.2.0"));
    }

    #[test]
    fn exempt_member_still_raises_the_group_version() {
        // Exemption suppresses the consistency failure but must not lower the
        // increment base, or `expand_plan` would move the exempt member back.
        let versions = BTreeMap::from([
            ("nm".to_string(), v("0.1.0")),
            ("nm_impl".to_string(), v("0.3.0")),
        ]);
        let exempt = HashSet::from(["nm_impl".to_string()]);
        let verdicts = groups().verdicts(&versions, &exempt);
        let nm = verdicts.get("nm").unwrap();
        assert!(nm.is_consistent());
        assert_eq!(nm.version().unwrap(), &v("0.3.0"));
    }

    #[test]
    fn closure_includes_every_member() {
        assert_eq!(groups().closure("nm_impl"), vec!["nm", "nm_impl"]);
        let empty = Groups::default();
        assert_eq!(empty.closure("events"), vec!["events"]);
    }

    /// A group whose members are all unpublishable or absent from the work tree
    /// has nothing to compare and no version to offer as an increment base.
    #[test]
    fn a_group_with_no_declared_member_is_consistent_and_versionless() {
        let verdicts = groups().verdicts(&BTreeMap::new(), &HashSet::new());
        let nm = verdicts.get("nm").unwrap();
        assert!(nm.is_consistent());
        assert_eq!(nm.version(), None);
        assert!(nm.members().is_empty());
    }
}
