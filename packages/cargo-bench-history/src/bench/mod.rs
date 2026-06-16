//! Benchmark-engine adapters: per-engine environment injection, the WSL env
//! propagation rule, and parsing of each engine's output into the model.
//!
//! Two engines are supported: Callgrind (via Gungraun, deterministic instruction
//! counts) and Criterion (wall-clock timings).

pub(crate) mod callgrind;
pub(crate) mod criterion;

pub(crate) use callgrind::parse_callgrind_summary;
pub(crate) use criterion::parse_criterion_case;

use crate::comparability::EngineSystem;

/// Directory under the cargo target root where Gungraun writes its summaries.
pub(crate) const GUNGRAUN_DIR: &str = "gungraun";

/// File name Gungraun writes for each benchmark case's machine-readable summary.
pub(crate) const SUMMARY_FILE: &str = "summary.json";

/// Directory under the cargo target root where Criterion writes its results.
pub(crate) const CRITERION_DIR: &str = "criterion";

/// Directory name Criterion gives the most recent run of each benchmark case.
pub(crate) const CRITERION_NEW_DIR: &str = "new";

/// File name Criterion writes describing a benchmark case's identity.
pub(crate) const CRITERION_BENCHMARK_FILE: &str = "benchmark.json";

/// File name Criterion writes with a benchmark case's statistical estimates.
pub(crate) const CRITERION_ESTIMATES_FILE: &str = "estimates.json";

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
/// an opaque wrapper such as `just bench-cg`), so every injected variable must
/// carry the `/u` "up" flag that shares it in the Windows→WSL direction. An
/// injected name already listed in `WSLENV` has the `u` flag added to its
/// existing flags if missing, rather than being left without it; other entries
/// are preserved verbatim. This is inert when no boundary is crossed and correct
/// when one is. Returns `None` when there is nothing to set.
pub(crate) fn merge_wslenv(existing: Option<&str>, injected_names: &[&str]) -> Option<String> {
    let mut entries: Vec<(String, String)> = Vec::new();

    if let Some(existing) = existing {
        for entry in existing.split(':').filter(|entry| !entry.is_empty()) {
            let (name, flags) = entry.split_once('/').map_or_else(
                || (entry.to_owned(), String::new()),
                |(name, flags)| (name.to_owned(), flags.to_owned()),
            );
            entries.push((name, flags));
        }
    }

    for injected in injected_names {
        if let Some((_, flags)) = entries.iter_mut().find(|(name, _)| name == injected) {
            if !flags.contains('u') {
                flags.push('u');
            }
        } else {
            entries.push(((*injected).to_owned(), "u".to_owned()));
        }
    }

    if entries.is_empty() {
        return None;
    }

    let rendered = entries
        .iter()
        .map(|(name, flags)| {
            if flags.is_empty() {
                name.clone()
            } else {
                format!("{name}/{flags}")
            }
        })
        .collect::<Vec<_>>()
        .join(":");
    Some(rendered)
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
    fn merge_wslenv_adds_up_flag_to_existing_entry_without_it() {
        assert_eq!(
            merge_wslenv(Some("GUNGRAUN_SAVE_SUMMARY/l"), &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("GUNGRAUN_SAVE_SUMMARY/lu".to_owned())
        );
    }

    #[test]
    fn merge_wslenv_adds_up_flag_to_existing_flagless_entry() {
        assert_eq!(
            merge_wslenv(Some("GUNGRAUN_SAVE_SUMMARY"), &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("GUNGRAUN_SAVE_SUMMARY/u".to_owned())
        );
    }

    #[test]
    fn merge_wslenv_preserves_unrelated_flagless_entries() {
        assert_eq!(
            merge_wslenv(Some("PATH"), &["GUNGRAUN_SAVE_SUMMARY"]),
            Some("PATH:GUNGRAUN_SAVE_SUMMARY/u".to_owned())
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
