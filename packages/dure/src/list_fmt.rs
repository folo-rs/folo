//! Formatting of `dure list` output.
//!
//! Ref: docs/design.md, "Listing sessions"; docs/implementation.md, "Output
//! rendering".

use crate::path_display::display_path;
use crate::session_record::SessionRecord;

/// Column headings, in the order they are printed.
const HEADERS: [&str; 5] = ["ID", "ATTACHED", "PID", "DIRECTORY", "COMMAND"];

/// Blank columns between one field and the next.
const COLUMN_GAP: usize = 2;

/// Renders live sessions as a stable table.
///
/// Column widths follow the widest cell in each column, so a heading lines up
/// with what it labels whatever the sessions happen to contain.
#[must_use]
pub(crate) fn format_list(sessions: &[SessionRecord]) -> String {
    let rows: Vec<[String; 5]> = sessions.iter().map(row).collect();
    let widths = column_widths(&rows);
    let mut lines = Vec::with_capacity(rows.len().saturating_add(1));
    lines.push(render_row(&HEADERS.map(str::to_string), &widths));
    for row in &rows {
        lines.push(render_row(row, &widths));
    }
    lines.join("\n")
}

fn row(session: &SessionRecord) -> [String; 5] {
    [
        session.id.to_string(),
        if session.attached { "yes" } else { "no" }.to_string(),
        session.supervisor_pid.to_string(),
        display_path(&session.launch_directory),
        session.command.join(" "),
    ]
}

/// Widest cell per column, headings included.
fn column_widths(rows: &[[String; 5]]) -> [usize; 5] {
    let mut widths = HEADERS.map(cell_width);
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell_width(cell));
        }
    }
    widths
}

/// Columns are counted in characters. Rendered width is a property of the
/// terminal and its font; a character count approximates it well enough for a
/// session list, and a byte count would not approximate it at all.
fn cell_width(cell: &str) -> usize {
    cell.chars().count()
}

/// Pads every cell but the last, so no line carries trailing blanks.
fn render_row(cells: &[String; 5], widths: &[usize; 5]) -> String {
    let mut line = String::new();
    let last = cells.len().saturating_sub(1);
    for (index, cell) in cells.iter().enumerate() {
        line.push_str(cell);
        if index == last {
            break;
        }
        let width = widths.get(index).copied().unwrap_or_default();
        let padding = width
            .saturating_sub(cell_width(cell))
            .saturating_add(COLUMN_GAP);
        for _ in 0..padding {
            line.push(' ');
        }
    }
    line
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn session(id: u32, directory: &str, command: &str) -> SessionRecord {
        SessionRecord {
            id,
            supervisor_pid: 99,
            supervisor_creation_time: 1,
            pipe_name: "pipe".to_string(),
            launch_directory: PathBuf::from(directory),
            command: vec![command.to_string()],
            started_at_unix_ms: 1,
            attached: true,
        }
    }

    #[test]
    fn empty_list_has_header_only() {
        let text = format_list(&[]);
        assert!(text.starts_with("ID  ATTACHED"));
        assert_eq!(text.lines().count(), 1);
    }

    #[test]
    fn includes_id_directory_and_command() {
        let mut only = session(4, "/work", "copilot.exe");
        only.command.push("--foo".to_string());
        let text = format_list(&[only]);
        assert!(text.contains('4'));
        assert!(text.contains("yes"));
        assert!(text.contains("99"));
        assert!(text.contains("/work"));
        assert!(text.contains("copilot.exe --foo"));
    }

    #[test]
    fn a_heading_starts_where_the_cell_it_labels_starts() {
        let text = format_list(&[session(1234567, "/work", "app.exe")]);
        let lines: Vec<&str> = text.lines().collect();
        let header = lines.first().expect("a header line");
        let row = lines.get(1).expect("a session line");
        // The id is wider than its heading here, so every later column shifts
        // right and the headings have to shift with it.
        assert_eq!(
            header.find("ATTACHED").expect("the attached heading"),
            row.find("yes").expect("the attached cell")
        );
        assert_eq!(
            header.find("PID").expect("the pid heading"),
            row.find("99").expect("the pid cell")
        );
        assert_eq!(
            header.find("DIRECTORY").expect("the directory heading"),
            row.find("/work").expect("the directory cell")
        );
        assert_eq!(
            header.find("COMMAND").expect("the command heading"),
            row.find("app.exe").expect("the command cell")
        );
    }

    #[test]
    fn a_column_is_as_wide_as_the_widest_session_in_it() {
        let text = format_list(&[
            session(1, "/a", "app.exe"),
            session(2, "/a-much-longer-directory", "app.exe"),
        ]);
        let lines: Vec<&str> = text.lines().collect();
        let header = lines.first().expect("a header line");
        let command = header.find("COMMAND").expect("the command heading");
        for row in lines.iter().skip(1) {
            assert_eq!(row.find("app.exe"), Some(command));
        }
    }

    #[test]
    fn directories_are_shown_without_the_extended_length_prefix() {
        let text = format_list(&[session(1, r"\\?\C:\Source", "app.exe")]);
        assert!(text.contains(r"C:\Source"));
        assert!(!text.contains(r"\\?\"));
    }

    #[test]
    fn no_line_carries_trailing_blanks() {
        let text = format_list(&[
            session(1, r"\\?\C:\a-very-long-source-directory", "copilot.exe"),
            session(2, r"\\?\C:\b", "app.exe"),
        ]);
        for line in text.lines() {
            assert_eq!(line, line.trim_end(), "trailing blanks in {line:?}");
        }
    }
}
