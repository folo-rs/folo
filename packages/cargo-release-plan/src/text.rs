// Shared message formatting helpers.

use std::fmt;
use std::fmt::Write as _;
use std::path::PathBuf;

/// Renders a repository-controlled path the way Git does.
///
/// Names containing quotes, backslashes, tabs, newlines, and other control
/// characters are all legal in Git and in a Cargo workspace, and the Git wrapper
/// deliberately preserves them. Emitting one verbatim would end a diff header
/// early, split a diagnostic across lines so the tail reads as a fresh GitHub
/// workflow command, or hand a terminal an escape sequence, so a name that needs
/// it is wrapped in the C-style quoting `git apply` and `patch` already
/// understand. Every rendering of a path a repository controls goes through
/// this.
pub(crate) fn quote_path(label: &str) -> String {
    if !label.chars().any(needs_quoting) {
        return label.to_string();
    }
    let mut quoted = String::with_capacity(label.len().saturating_add(2));
    quoted.push('"');
    for character in label.chars() {
        match character {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            control if control.is_ascii_control() => {
                write!(quoted, "\\{:03o}", u32::from(control)).expect("writing to String");
            }
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted
}

/// Whether a character forces the whole path into quoted form.
///
/// Non-ASCII characters are left alone: the artifacts are UTF-8 and escaping
/// them would only make an ordinary accented file name unreadable.
fn needs_quoting(character: char) -> bool {
    character == '"' || character == '\\' || character.is_ascii_control()
}

/// Applies [`quote_path`] to a value an error message names.
///
/// `#[ohno::error]` scopes each positional `#[display(...)]` argument to `self`,
/// so an error message can only reach the escaping through a method on the field
/// it renders.
pub(crate) trait Quotable {
    fn quoted(&self) -> String;
}

impl Quotable for String {
    fn quoted(&self) -> String {
        quote_path(self)
    }
}

impl Quotable for PathBuf {
    fn quoted(&self) -> String {
        quote_path(&self.to_string_lossy())
    }
}

/// Renders a count with a noun that agrees with it.
///
/// Diagnostics report counts that are only known at runtime, and the workspace
/// prefers agreeing prose over `package(s)` forms, so every such message routes
/// through this helper instead of choosing a form at the call site.
pub(crate) fn plural(count: usize, singular: &str) -> impl fmt::Display {
    Plural {
        count,
        singular: singular.to_string(),
    }
}

/// Matches the user-facing short-commit convention in `cbh_detect`.
///
/// Ref: `packages/cbh_detect/src/detect/findings.rs`, `short_commit`.
const SHORT_COMMIT_LEN: usize = 12;

pub(crate) fn short_commit(commit: &str) -> &str {
    commit
        .get(..commit.len().min(SHORT_COMMIT_LEN))
        .unwrap_or(commit)
}

struct Plural {
    count: usize,
    singular: String,
}

impl fmt::Display for Plural {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.count, self.singular)?;
        if self.count != 1 {
            // Every noun this crate pluralizes takes a bare `s`.
            f.write_str("s")?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn agrees_with_the_count() {
        assert_eq!(plural(0, "package").to_string(), "0 packages");
        assert_eq!(plural(1, "package").to_string(), "1 package");
        assert_eq!(plural(2, "manifest").to_string(), "2 manifests");
    }

    #[test]
    fn short_commit_truncates_long_revisions() {
        assert_eq!(short_commit("abcdefghijklmnop"), "abcdefghijkl");
        assert_eq!(short_commit("abc"), "abc");
    }

    #[test]
    fn an_ordinary_path_is_not_quoted() {
        assert_eq!(quote_path("a/src/lib.rs"), "a/src/lib.rs");
        // Non-ASCII is ordinary here even though Git would escape it by default.
        assert_eq!(quote_path("a/src/lïb.rs"), "a/src/lïb.rs");
    }

    #[test]
    fn an_unusual_path_is_quoted_including_its_prefix() {
        assert_eq!(quote_path("a/od\"d\\name"), r#""a/od\"d\\name""#);
        assert_eq!(quote_path("a/tab\there"), r#""a/tab\there""#);
        assert_eq!(quote_path("a/nl\nhere"), r#""a/nl\nhere""#);
        assert_eq!(quote_path("a/cr\rhere"), r#""a/cr\rhere""#);
        assert_eq!(quote_path("a/bell\u{7}"), r#""a/bell\007""#);
    }
}
