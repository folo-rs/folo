//! Formatting of `dure list` output.

use crate::session_record::SessionRecord;

/// Renders live sessions as a stable table.
#[must_use]
pub(crate) fn format_list(sessions: &[SessionRecord]) -> String {
    let mut lines = Vec::new();
    lines.push("ID  ATTACHED  PID    DIRECTORY  COMMAND".to_string());
    for session in sessions {
        let attached = if session.attached { "yes" } else { "no" };
        let command = session.command.join(" ");
        lines.push(format!(
            "{}  {attached}  {}  {}  {command}",
            session.id,
            session.supervisor_pid,
            session.launch_directory.display()
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn empty_list_has_header_only() {
        let text = format_list(&[]);
        assert!(text.starts_with("ID  ATTACHED"));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn includes_id_directory_and_command() {
        let session = SessionRecord {
            id: 4,
            supervisor_pid: 99,
            supervisor_creation_time: 1,
            pipe_name: "pipe".to_string(),
            launch_directory: PathBuf::from("/work"),
            command: vec!["copilot.exe".to_string(), "--foo".to_string()],
            started_at_unix_ms: 1,
            attached: true,
        };
        let text = format_list(&[session]);
        assert!(text.contains('4'));
        assert!(text.contains("yes"));
        assert!(text.contains("99"));
        assert!(text.contains("/work"));
        assert!(text.contains("copilot.exe --foo"));
    }
}
