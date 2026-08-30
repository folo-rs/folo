// Resolved dependency closures read from Cargo lockfiles.
//
// Cargo puts a lockfile into every archive it builds, and that lockfile carries
// the packaged crate's own resolved closure rather than the whole workspace's.
// A consumer installing a binary with `cargo install --locked` builds from it,
// so the closure is part of what that consumer receives.
// Ref: docs/design.md, "Lockfiles of binary packages".

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ohno::AppError;
use toml_edit::{DocumentMut, Item};

use crate::MalformedLockfileError;

/// A package's resolved dependencies, keyed by crate name.
///
/// A name maps to a set because one closure may legitimately hold several
/// versions of the same crate, and because comparing per name is what lets a
/// report say which dependency moved.
pub(crate) type Closure = BTreeMap<String, BTreeSet<String>>;

/// The locked packages of one Cargo lockfile, indexed for closure walks.
///
/// Only what a closure walk needs is retained: which entries exist, how each is
/// identified, and which entries each names as a dependency. Ref:
/// docs/implementation.md, "Lockfile closures".
#[derive(Debug)]
pub(crate) struct Lockfile {
    entries: Vec<LockEntry>,
}

impl Lockfile {
    /// Parses a lockfile, naming `label` in any diagnostic.
    pub(crate) fn parse(text: &str, label: &str) -> Result<Self, AppError> {
        let doc: DocumentMut = text
            .parse()
            .map_err(|error| MalformedLockfileError::caused_by(label, error))?;
        // A lockfile that resolved to nothing omits the array entirely, which is
        // a well-formed lockfile with an empty closure rather than a fault.
        let Some(tables) = doc.get("package").and_then(Item::as_array_of_tables) else {
            return Ok(Self {
                entries: Vec::new(),
            });
        };
        let mut entries = Vec::with_capacity(tables.len());
        for table in tables {
            let Some(name) = table.get("name").and_then(Item::as_str) else {
                return Err(MalformedLockfileError::new(label).into());
            };
            let Some(version) = table.get("version").and_then(Item::as_str) else {
                return Err(MalformedLockfileError::new(label).into());
            };
            let mut dependencies = Vec::new();
            if let Some(item) = table.get("dependencies") {
                let Some(array) = item.as_array() else {
                    return Err(MalformedLockfileError::new(label).into());
                };
                for value in array {
                    let Some(text) = value.as_str() else {
                        return Err(MalformedLockfileError::new(label).into());
                    };
                    dependencies.push(DepRef::parse(text));
                }
            }
            entries.push(LockEntry {
                name: name.to_owned(),
                version: version.to_owned(),
                source: table
                    .get("source")
                    .and_then(Item::as_str)
                    .map(ToOwned::to_owned),
                dependencies,
            });
        }
        Ok(Self { entries })
    }

    /// The locked identities `root` transitively depends on.
    ///
    /// Returns `None` when the lockfile does not resolve `root` at all, which is
    /// what a lockfile predating the package looks like.
    ///
    /// The root's own entry is left out however the walk reaches it. Its version
    /// is the declared version the invariant already tracks, so counting it
    /// would make every increment look like a further change and leave the
    /// package permanently unable to settle.
    pub(crate) fn closure(&self, root: &str) -> Option<Closure> {
        let root_index = self.root_index(root)?;
        let mut seen = BTreeSet::from([root_index]);
        let mut queue = VecDeque::from([root_index]);
        let mut closure = Closure::new();
        while let Some(index) = queue.pop_front() {
            let entry = self
                .entries
                .get(index)
                .expect("every queued index was produced by a lookup into this same vector");
            if index != root_index {
                closure
                    .entry(entry.name.clone())
                    .or_default()
                    .insert(entry.identity());
            }
            for dep in &entry.dependencies {
                for next in self.matching(dep) {
                    if seen.insert(next) {
                        queue.push_back(next);
                    }
                }
            }
        }
        Some(closure)
    }

    /// Locates the entry standing for the workspace member being classified.
    ///
    /// A registry crate may share a workspace member's name, and only the member
    /// is the package being published. Cargo records no `source` for a package
    /// it resolved from a path, which is what tells the two apart.
    fn root_index(&self, root: &str) -> Option<usize> {
        let named = || {
            self.entries
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.name == root)
        };
        named()
            .find(|(_, entry)| entry.source.is_none())
            .or_else(|| named().next())
            .map(|(index, _)| index)
    }

    /// Indices of the entries a dependency reference names.
    fn matching(&self, dep: &DepRef) -> Vec<usize> {
        self.entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| {
                entry.name == dep.name
                    && dep
                        .version
                        .as_ref()
                        .is_none_or(|version| &entry.version == version)
            })
            .map(|(index, _)| index)
            .collect()
    }
}

/// One `[[package]]` entry of a lockfile.
#[derive(Debug)]
struct LockEntry {
    name: String,
    version: String,
    /// Where the package was resolved from; absent for a path dependency.
    source: Option<String>,
    dependencies: Vec<DepRef>,
}

impl LockEntry {
    /// How this entry is identified when closures are compared.
    ///
    /// The source is carried because a `[patch]` can redirect a name and version
    /// at an entirely different tree. The checksum is not, because a registry
    /// fixes it for a given name, version and source.
    fn identity(&self) -> String {
        match &self.source {
            Some(source) => format!("{} ({source})", self.version),
            None => self.version.clone(),
        }
    }
}

/// A dependency named by a lockfile entry.
///
/// Cargo spells a dependency as a bare name while the name is unambiguous and
/// adds the version once it is not, so the version is optional here rather than
/// a property of the lockfile format version.
#[derive(Debug)]
struct DepRef {
    name: String,
    version: Option<String>,
}

impl DepRef {
    fn parse(text: &str) -> Self {
        let mut parts = text.split_whitespace();
        Self {
            name: parts.next().unwrap_or_default().to_owned(),
            // Lockfiles written before version 3 append the source in
            // parentheses, which names no entry the walk needs.
            version: parts.next().map(ToOwned::to_owned),
        }
    }
}

/// How one dependency differs between two closures.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClosureChange {
    Added,
    Deleted,
    Modified,
}

impl ClosureChange {
    /// The vocabulary `report.json` uses for a change of any source.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Modified => "modified",
        }
    }
}

/// Dependencies whose locked identities differ, in name order.
pub(crate) fn closure_changes(anchor: &Closure, work: &Closure) -> Vec<(String, ClosureChange)> {
    let names: BTreeSet<&String> = anchor.keys().chain(work.keys()).collect();
    names
        .into_iter()
        .filter_map(|name| {
            let change = match (anchor.get(name), work.get(name)) {
                (None, Some(_)) => ClosureChange::Added,
                (Some(_), None) => ClosureChange::Deleted,
                (Some(before), Some(after)) if before != after => ClosureChange::Modified,
                _ => return None,
            };
            Some((name.clone(), change))
        })
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const LABEL: &str = "Cargo.lock";

    fn closure_of(text: &str, root: &str) -> Closure {
        Lockfile::parse(text, LABEL)
            .unwrap()
            .closure(root)
            .expect("the fixtures all resolve the root they are asked about")
    }

    #[test]
    fn a_lockfile_without_packages_has_an_empty_closure() {
        let lockfile = Lockfile::parse("version = 4\n", LABEL).unwrap();
        assert!(lockfile.closure("tool").is_none());
    }

    #[test]
    fn an_unresolved_root_has_no_closure() {
        let text = "\
[[package]]
name = \"other\"
version = \"1.0.0\"
";
        assert!(
            Lockfile::parse(text, LABEL)
                .unwrap()
                .closure("tool")
                .is_none()
        );
    }

    #[test]
    fn a_closure_follows_dependencies_transitively() {
        let text = "\
[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"direct\"]

[[package]]
name = \"direct\"
version = \"1.0.0\"
source = \"registry+https://example.invalid\"
dependencies = [\"indirect\"]

[[package]]
name = \"indirect\"
version = \"2.0.0\"
source = \"registry+https://example.invalid\"

[[package]]
name = \"unrelated\"
version = \"9.0.0\"
source = \"registry+https://example.invalid\"
";
        let closure = closure_of(text, "tool");
        assert_eq!(
            closure.keys().collect::<Vec<_>>(),
            vec!["direct", "indirect"]
        );
        assert!(!closure.contains_key("unrelated"));
    }

    #[test]
    fn a_closure_excludes_the_root_itself() {
        // Otherwise incrementing the package would register as a change to its
        // own dependencies and it could never reach a settled state.
        let text = "\
[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"helper\"]

[[package]]
name = \"helper\"
version = \"1.0.0\"
dependencies = [\"tool\"]
";
        let closure = closure_of(text, "tool");
        assert_eq!(closure.keys().collect::<Vec<_>>(), vec!["helper"]);
    }

    #[test]
    fn a_dependency_cycle_terminates() {
        let text = "\
[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"a\"]

[[package]]
name = \"a\"
version = \"1.0.0\"
dependencies = [\"b\"]

[[package]]
name = \"b\"
version = \"1.0.0\"
dependencies = [\"a\"]
";
        let closure = closure_of(text, "tool");
        assert_eq!(closure.keys().collect::<Vec<_>>(), vec!["a", "b"]);
    }

    #[test]
    fn a_disambiguated_dependency_selects_one_version() {
        let text = "\
[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"dup 2.0.0\"]

[[package]]
name = \"dup\"
version = \"1.0.0\"
source = \"registry+https://example.invalid\"

[[package]]
name = \"dup\"
version = \"2.0.0\"
source = \"registry+https://example.invalid\"
";
        let closure = closure_of(text, "tool");
        let versions = closure.get("dup").unwrap();
        assert_eq!(versions.len(), 1);
        assert!(
            versions
                .iter()
                .any(|identity| identity.starts_with("2.0.0"))
        );
    }

    #[test]
    fn a_workspace_member_wins_over_a_registry_crate_of_the_same_name() {
        // The published package is the member, so the walk must start there.
        let text = "\
[[package]]
name = \"tool\"
version = \"9.9.9\"
source = \"registry+https://example.invalid\"

[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"helper\"]

[[package]]
name = \"helper\"
version = \"1.0.0\"
";
        let closure = closure_of(text, "tool");
        assert_eq!(closure.keys().collect::<Vec<_>>(), vec!["helper"]);
    }

    #[test]
    fn a_source_change_is_part_of_an_identity() {
        // A patched dependency keeps its name and version while resolving to a
        // different tree entirely.
        let plain = "\
[[package]]
name = \"tool\"
version = \"0.1.0\"
dependencies = [\"dep\"]

[[package]]
name = \"dep\"
version = \"1.0.0\"
source = \"registry+https://example.invalid\"
";
        let patched = plain.replace("example.invalid", "elsewhere.invalid");
        let changes = closure_changes(&closure_of(plain, "tool"), &closure_of(&patched, "tool"));
        assert_eq!(changes, vec![("dep".to_owned(), ClosureChange::Modified)]);
    }

    #[test]
    fn closure_changes_name_what_moved() {
        let before = Closure::from([
            ("kept".to_owned(), BTreeSet::from(["1.0.0".to_owned()])),
            ("bumped".to_owned(), BTreeSet::from(["1.0.0".to_owned()])),
            ("dropped".to_owned(), BTreeSet::from(["1.0.0".to_owned()])),
        ]);
        let after = Closure::from([
            ("kept".to_owned(), BTreeSet::from(["1.0.0".to_owned()])),
            ("bumped".to_owned(), BTreeSet::from(["1.1.0".to_owned()])),
            ("gained".to_owned(), BTreeSet::from(["1.0.0".to_owned()])),
        ]);
        assert_eq!(
            closure_changes(&before, &after),
            vec![
                ("bumped".to_owned(), ClosureChange::Modified),
                ("dropped".to_owned(), ClosureChange::Deleted),
                ("gained".to_owned(), ClosureChange::Added),
            ]
        );
    }

    #[test]
    fn identical_closures_report_nothing() {
        let closure = Closure::from([("dep".to_owned(), BTreeSet::from(["1.0.0".to_owned()]))]);
        assert!(closure_changes(&closure, &closure).is_empty());
    }

    #[test]
    fn a_change_renders_the_vocabulary_reports_use() {
        assert_eq!(ClosureChange::Added.as_str(), "added");
        assert_eq!(ClosureChange::Deleted.as_str(), "deleted");
        assert_eq!(ClosureChange::Modified.as_str(), "modified");
    }

    #[test]
    fn a_malformed_lockfile_is_rejected() {
        for text in [
            "not = = toml",
            "[[package]]\nversion = \"1.0.0\"\n",
            "[[package]]\nname = \"a\"\n",
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = \"b\"\n",
            "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\ndependencies = [1]\n",
        ] {
            let error = Lockfile::parse(text, LABEL).unwrap_err();
            assert!(error.find_source::<MalformedLockfileError>().is_some());
        }
    }
}
