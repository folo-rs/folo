//! Auto-detect a session from the current directory.
//!
//! Ref: docs/design.md, "Auto-detect".

use std::path::Path;

use crate::session_id::SessionId;
use crate::session_record::SessionRecord;

/// Result of matching live sessions against the current directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum DetectOutcome {
    /// No live sessions exist.
    None,
    /// Exactly one live session was launched from the current directory.
    Unique(SessionId),
    /// Zero or several matches for this directory; the caller lists and prompts.
    Ambiguous(Vec<SessionRecord>),
}

/// Chooses a session using the launch-directory rule.
///
/// `cwd` must already be the PAL-canonicalized current directory. Comparison is
/// equality of those canonical paths.
pub(crate) fn auto_detect(live: &[SessionRecord], cwd: &Path, verbose: bool) -> DetectOutcome {
    if live.is_empty() {
        verbose_note(verbose, "auto-detect: no live sessions");
        return DetectOutcome::None;
    }

    let matches: Vec<&SessionRecord> = live
        .iter()
        .filter(|session| session.launch_directory == cwd)
        .collect();

    if verbose {
        eprintln!(
            "auto-detect: current directory {}; {} live {}; {} launch-directory {}",
            cwd.display(),
            live.len(),
            session_noun(live.len()),
            matches.len(),
            match_noun(matches.len()),
        );
        for session in live {
            let mark = if session.launch_directory == cwd {
                "match"
            } else {
                "different directory"
            };
            eprintln!(
                "auto-detect: session {} dir {} ({mark})",
                session.id,
                session.launch_directory.display()
            );
        }
    }

    match matches.as_slice() {
        [only] => DetectOutcome::Unique(only.session_id()),
        _ => DetectOutcome::Ambiguous(live.to_vec()),
    }
}

// English pluralization is not a behavioral contract.
#[cfg_attr(test, mutants::skip)]
fn session_noun(count: usize) -> &'static str {
    if count == 1 { "session" } else { "sessions" }
}

// English pluralization is not a behavioral contract.
#[cfg_attr(test, mutants::skip)]
fn match_noun(count: usize) -> &'static str {
    if count == 1 { "match" } else { "matches" }
}

// Verbose log text is not a behavioral contract.
#[cfg_attr(test, mutants::skip)]
fn verbose_note(verbose: bool, message: &str) {
    if verbose {
        eprintln!("{message}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn record(id: u32, dir: &str) -> SessionRecord {
        SessionRecord {
            id,
            supervisor_pid: 1,
            supervisor_creation_time: 1,
            pipe_name: format!("pipe-{id}"),
            launch_directory: PathBuf::from(dir),
            command: vec!["app.exe".to_string()],
            started_at_unix_ms: 1,
            attached: false,
        }
    }

    #[test]
    fn no_sessions_fails() {
        assert_eq!(
            auto_detect(&[], Path::new("/work"), false),
            DetectOutcome::None
        );
    }

    #[test]
    fn unique_launch_directory_match() {
        let live = [record(1, "/a"), record(2, "/work")];
        assert_eq!(
            auto_detect(&live, Path::new("/work"), false),
            DetectOutcome::Unique(SessionId::from_u32(2).unwrap())
        );
    }

    #[test]
    fn verbose_unique_match_is_still_unique() {
        let live = [record(1, "/work")];
        assert_eq!(
            auto_detect(&live, Path::new("/work"), true),
            DetectOutcome::Unique(SessionId::from_u32(1).unwrap())
        );
    }

    #[test]
    fn several_matches_are_ambiguous() {
        let live = [record(1, "/work"), record(2, "/work")];
        let outcome = auto_detect(&live, Path::new("/work"), false);
        assert!(matches!(outcome, DetectOutcome::Ambiguous(_)));
    }

    #[test]
    fn single_session_in_other_directory_is_ambiguous() {
        let live = [record(1, "/other")];
        let outcome = auto_detect(&live, Path::new("/work"), false);
        assert!(matches!(outcome, DetectOutcome::Ambiguous(sessions) if sessions.len() == 1));
    }
}
