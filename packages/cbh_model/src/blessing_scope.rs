//! Persistent acceptance scopes, independent of observed measurement partitions.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::DiscriminantSet;

/// The discriminant restrictions on a persisted blessing.
///
/// Each empty axis is unrestricted, including partitions not yet recorded.
/// Values within an axis are alternatives; every axis must match. Matching is
/// ASCII-case-insensitive, like query discriminant filters.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BlessingScope {
    /// Accepted engines, or every engine when empty.
    pub engine: Vec<String>,
    /// Accepted target triples, or every target when empty.
    pub target_triple: Vec<String>,
    /// Accepted machine keys, or every machine when empty.
    pub machine_key: Vec<String>,
}

impl BlessingScope {
    /// Whether the scope accepts this concrete measurement partition.
    #[must_use]
    pub fn matches(&self, set: &DiscriminantSet) -> bool {
        axis_matches(&self.engine, set.engine.as_str())
            && axis_matches(&self.target_triple, set.target_triple.as_str())
            && axis_matches(&self.machine_key, set.machine_key.as_str())
    }

    /// Whether any partition can belong to both scopes.
    #[must_use]
    pub fn intersects(&self, other: &Self) -> bool {
        axes_intersect(&self.engine, &other.engine)
            && axes_intersect(&self.target_triple, &other.target_triple)
            && axes_intersect(&self.machine_key, &other.machine_key)
    }

    /// Whether every partition in `other` belongs to this scope.
    ///
    /// Deletion uses containment rather than intersection so a narrowed request
    /// cannot revoke acceptance outside its selected scope.
    #[must_use]
    pub fn contains(&self, other: &Self) -> bool {
        axis_contains(&self.engine, &other.engine)
            && axis_contains(&self.target_triple, &other.target_triple)
            && axis_contains(&self.machine_key, &other.machine_key)
    }

    /// Display values in engine, target-triple, machine-key order.
    #[must_use]
    pub fn labels(&self) -> [String; 3] {
        [&self.engine, &self.target_triple, &self.machine_key].map(|axis| {
            if axis.is_empty() {
                "all".to_owned()
            } else {
                axis.join("|")
            }
        })
    }
}

impl From<&DiscriminantSet> for BlessingScope {
    fn from(set: &DiscriminantSet) -> Self {
        Self {
            engine: vec![set.engine.to_string()],
            target_triple: vec![set.target_triple.to_string()],
            machine_key: vec![set.machine_key.to_string()],
        }
    }
}

impl fmt::Display for BlessingScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.labels().join("/"))
    }
}

fn axes_intersect(left: &[String], right: &[String]) -> bool {
    right.is_empty() || left.is_empty() || right.iter().any(|right| axis_matches(left, right))
}

fn axis_contains(outer: &[String], inner: &[String]) -> bool {
    outer.is_empty() || (!inner.is_empty() && inner.iter().all(|inner| axis_matches(outer, inner)))
}

fn axis_matches(values: &[String], actual: &str) -> bool {
    values.is_empty()
        || values
            .iter()
            .any(|value| value.eq_ignore_ascii_case(actual))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use super::*;
    use crate::Engine;

    static_assertions::assert_impl_all!(BlessingScope: Send, Sync, UnwindSafe, RefUnwindSafe);

    #[test]
    fn scope_intersection_and_containment_are_distinct() {
        let all = BlessingScope::default();
        let engine = BlessingScope {
            engine: vec!["CALLGRIND".to_owned()],
            ..all.clone()
        };
        let partition = DiscriminantSet::new(Engine::Callgrind, &"target".into(), &"m1".into());
        let concrete = BlessingScope::from(&partition);
        assert!(engine.matches(&partition));
        assert!(all.contains(&engine));
        assert!(!engine.contains(&all));
        assert!(engine.contains(&concrete));
        assert!(!concrete.contains(&engine));
        assert!(concrete.intersects(&engine));
        assert!(engine.intersects(&concrete));
        assert_eq!(all.labels(), ["all", "all", "all"]);
        assert_eq!(concrete.to_string(), "callgrind/target/m1");

        for scope in [
            BlessingScope {
                engine: vec!["criterion".to_owned()],
                ..all.clone()
            },
            BlessingScope {
                target_triple: vec!["other".to_owned()],
                ..all.clone()
            },
            BlessingScope {
                machine_key: vec!["m2".to_owned()],
                ..all
            },
        ] {
            assert!(!scope.matches(&partition));
            assert!(!scope.contains(&concrete));
        }
    }

    #[test]
    fn repeated_values_are_alternatives_and_missing_axes_are_not_defaulted_on_read() {
        let scope = BlessingScope {
            machine_key: vec!["m1".to_owned(), "m2".to_owned()],
            ..BlessingScope::default()
        };
        let subset = BlessingScope {
            machine_key: vec!["M2".to_owned()],
            ..BlessingScope::default()
        };
        assert!(scope.contains(&subset));
        assert!(!subset.contains(&scope));
        assert_eq!(scope.labels(), ["all", "all", "m1|m2"]);
        assert_eq!(
            serde_json::from_str::<BlessingScope>(&serde_json::to_string(&scope).unwrap()).unwrap(),
            scope
        );
        serde_json::from_str::<BlessingScope>(r#"{"engine":[]}"#).unwrap_err();
    }
}
