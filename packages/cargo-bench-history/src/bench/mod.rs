//! Benchmark-engine adapters: per-engine environment injection, the WSL env
//! propagation rule, and parsing of each engine's output into the model.
//!
//! Iteration 1 implements the Callgrind (Gungraun) engine; Criterion follows in a
//! later iteration.

pub(crate) mod callgrind;

use std::collections::HashSet;

pub(crate) use callgrind::parse_callgrind_summary;

use crate::comparability::EngineSystem;

/// Directory under the cargo target root where Gungraun writes its summaries.
pub(crate) const GUNGRAUN_DIR: &str = "gungraun";

/// File name Gungraun writes for each benchmark case's machine-readable summary.
pub(crate) const SUMMARY_FILE: &str = "summary.json";

/// The environment variables to inject so an engine emits machine-readable output.
///
/// Callgrind needs `GUNGRAUN_SAVE_SUMMARY=pretty-json` so Gungraun writes the
/// `summary.json` files the tool harvests. Criterion writes `estimates.json`
/// unconditionally, so it needs nothing.
pub(crate) fn injected_env(engine: EngineSystem) -> Vec<(String, String)> {
    match engine {
        EngineSystem::Callgrind => {
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        }
        EngineSystem::Criterion => Vec::new(),
    }
}

/// Computes the `WSLENV` value that propagates injected variables across a WSL
/// boundary, preserving any existing entries.
///
/// The tool cannot reliably detect whether a command crosses into WSL (it may be
/// an opaque wrapper such as `just bench-cg`), so it always appends the injected
/// variable names with the `/u` "up" flag. This is inert when no boundary is
/// crossed and correct when one is. Returns `None` when there is nothing to set.
pub(crate) fn merge_wslenv(existing: Option<&str>, injected_names: &[&str]) -> Option<String> {
    let mut entries: Vec<String> = Vec::new();
    let mut present: HashSet<&str> = HashSet::new();

    if let Some(existing) = existing {
        for entry in existing.split(':').filter(|entry| !entry.is_empty()) {
            let name = entry.split('/').next().unwrap_or(entry);
            present.insert(name);
            entries.push(entry.to_owned());
        }
    }

    for name in injected_names {
        if present.contains(name) {
            continue;
        }
        entries.push(format!("{name}/u"));
    }

    if entries.is_empty() {
        None
    } else {
        Some(entries.join(":"))
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn callgrind_injects_the_summary_flag() {
        let env = injected_env(EngineSystem::Callgrind);
        assert_eq!(
            env,
            vec![("GUNGRAUN_SAVE_SUMMARY".to_owned(), "pretty-json".to_owned())]
        );
    }

    #[test]
    fn criterion_injects_nothing() {
        assert!(injected_env(EngineSystem::Criterion).is_empty());
    }

    #[test]
    fn merge_wslenv_from_empty() {
        assert_eq!(
            merge_wslenv(None, &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("GUNGRAUN_SAVE_SUMMARY/u".to_owned())
        );
    }

    #[test]
    fn merge_wslenv_appends_to_existing() {
        assert_eq!(
            merge_wslenv(Some("PATH/l"), &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("PATH/l:GUNGRAUN_SAVE_SUMMARY/u".to_owned())
        );
    }

    #[test]
    fn merge_wslenv_does_not_duplicate() {
        assert_eq!(
            merge_wslenv(Some("GUNGRAUN_SAVE_SUMMARY/u"), &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("GUNGRAUN_SAVE_SUMMARY/u".to_owned())
        );
    }

    #[test]
    fn merge_wslenv_nothing_to_set() {
        assert_eq!(merge_wslenv(None, &[]), None);
    }

    #[test]
    fn merge_wslenv_keeps_existing_when_no_injection() {
        assert_eq!(merge_wslenv(Some("FOO/u"), &[]), Some("FOO/u".to_owned()));
    }
}
