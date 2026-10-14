//! Escaping at human-output boundaries keeps identities as data, not document structure.

/// Escapes arbitrary text for Markdown prose, headings and table cells.
pub(crate) fn markdown_text(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '|' => escaped.push_str("&#124;"),
            '\n' | '\r' => escaped.push(' '),
            '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '~' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            ch if ch.is_control() => escaped.extend(ch.escape_default()),
            ch => escaped.push(ch),
        }
    }
    escaped
}

/// HTML summary labels use literal text, not Markdown backslash escapes.
pub(crate) fn html_text(value: &str) -> String {
    plain_text(value)
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Uses a code span for identities, with a delimiter longer than any embedded run.
pub(crate) fn code_span(value: &str) -> String {
    let value = plain_text(value);
    let longest = value.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.saturating_add(1));
    if value.contains('`') || value.starts_with(' ') || value.ends_with(' ') {
        format!("{fence} {value} {fence}")
    } else {
        format!("{fence}{value}{fence}")
    }
}

/// Displays identities on one terminal line without interpreting control sequences.
pub(crate) fn plain_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch.is_control() {
            escaped.extend(ch.escape_default());
        } else {
            escaped.push(ch);
        }
    }
    escaped
}

/// Fences arbitrary content without letting an embedded backtick run end the block.
pub(crate) fn code_block(language: &str, value: &str) -> String {
    let longest = value.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat(longest.saturating_add(1).max(3));
    format!("{fence}{language}\n{value}\n{fence}")
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn identities_cannot_add_rows_html_or_terminal_controls() {
        assert_eq!(
            markdown_text("a|<details>\n`b`"),
            "a&#124;&lt;details&gt; \\`b\\`"
        );
        assert_eq!(plain_text("a\n\u{1b}[31m"), r"a\n\u{1b}[31m");
        assert_eq!(code_block("text", "```\nx"), "````text\n```\nx\n````");
    }

    #[test]
    fn markdown_and_html_preserve_literal_identity_punctuation() {
        let identity = "_case_*`<&>\n";
        assert_eq!(markdown_text(identity), "\\_case\\_\\*\\`&lt;&amp;&gt; ");
        assert_eq!(html_text(identity), "_case_*`&lt;&amp;&gt;\\n");
    }
}
