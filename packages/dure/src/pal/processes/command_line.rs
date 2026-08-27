//! Windows `CreateProcessW` command-line construction.
//!
//! `CreateProcessW` takes one string that `CommandLineToArgvW` later splits.
//! Each argv element is quoted so spaces and embedded quotes survive that split.
//! See <https://learn.microsoft.com/windows/win32/api/shellapi/nf-shellapi-commandlinetoargvw>.

// Used by the Windows process PAL. Unit tests cover quoting on every target.
#![cfg_attr(
    not(any(windows, test)),
    expect(dead_code, reason = "CreateProcessW quoting is Windows PAL plus tests")
)]

/// Builds a Windows process command line from an executable and following argv.
#[must_use]
pub(crate) fn windows_command_line(exe: &str, args: &[String]) -> String {
    let mut line = quote_windows_arg(exe);
    for arg in args {
        line.push(' ');
        line.push_str(&quote_windows_arg(arg));
    }
    line
}

/// Quotes one argv element for `CommandLineToArgvW`.
///
/// Empty arguments become `""`. Arguments without whitespace or quotes are
/// copied unchanged. Otherwise the argument is wrapped in quotes and internal
/// quotes are escaped by doubling preceding backslashes, matching the Windows
/// argv rules.
#[must_use]
pub(crate) fn quote_windows_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }
    let needs_quotes = arg
        .bytes()
        .any(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'"'));
    if !needs_quotes {
        return arg.to_string();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0_usize;
    for ch in arg.chars() {
        if ch == '\\' {
            backslashes = backslashes.saturating_add(1);
            continue;
        }
        if ch == '"' {
            quoted.extend(std::iter::repeat_n(
                '\\',
                backslashes.saturating_mul(2).saturating_add(1),
            ));
            quoted.push('"');
            backslashes = 0;
            continue;
        }
        quoted.extend(std::iter::repeat_n('\\', backslashes));
        quoted.push(ch);
        backslashes = 0;
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes.saturating_mul(2)));
    quoted.push('"');
    quoted
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn plain_args_are_unquoted() {
        assert_eq!(quote_windows_arg("app.exe"), "app.exe");
        assert_eq!(
            windows_command_line("app.exe", &["--foo".to_string()]),
            "app.exe --foo"
        );
    }

    #[test]
    fn spaces_and_quotes_are_escaped() {
        assert_eq!(quote_windows_arg(""), "\"\"");
        assert_eq!(quote_windows_arg("a b"), "\"a b\"");
        assert_eq!(quote_windows_arg(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quote_windows_arg(r"C:\dir\"), r"C:\dir\");
        assert_eq!(
            quote_windows_arg(r"C:\Program Files\"),
            r#""C:\Program Files\\""#
        );
    }
}
