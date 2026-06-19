//! Sanitizing arbitrary strings into filesystem-safe file name components.

/// Converts an arbitrary string into a filesystem-safe file name component.
///
/// Every character that is not an ASCII alphanumeric or one of `.`, `_`, `-`
/// is replaced with `_`, and trailing dots (which Windows silently strips) are
/// removed. The result never contains path separators, so it is always a single
/// path component.
///
/// A leading `_` is prepended when the result would otherwise be unusable as a
/// file name: an empty string (which includes inputs like `.` and `..` once
/// their dots are stripped) or a name reserved by Windows for a device (such as
/// `CON`, `NUL`, `COM1`, or `LPT1`, case-insensitively and regardless of any
/// extension).
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    let mut sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();

    // Windows silently strips trailing dots from file names, which would let
    // distinct operation names collide and produce surprising paths.
    while sanitized.ends_with('.') {
        sanitized.pop();
    }

    if sanitized.is_empty() || is_windows_reserved(&sanitized) {
        sanitized.insert(0, '_');
    }

    sanitized
}

/// Returns whether the name (ignoring any extension) is a reserved Windows
/// device name, which is rejected by the OS even when an extension is appended.
fn is_windows_reserved(name: &str) -> bool {
    const RESERVED: [&str; 4] = ["CON", "PRN", "AUX", "NUL"];

    let stem = name.split_once('.').map_or(name, |(stem, _extension)| stem);

    if RESERVED
        .iter()
        .any(|reserved| stem.eq_ignore_ascii_case(reserved))
    {
        return true;
    }

    // Escape `COM0`..=`COM9` and `LPT0`..=`LPT9`. The canonical reserved set is
    // `COM1`..=`COM9` and `LPT1`..=`LPT9`, but the `0` variants are escaped too as
    // a conservative choice: over-escaping a name is harmless, whereas failing to
    // escape a genuinely reserved name makes the file unwritable on Windows.
    for prefix in ["COM", "LPT"] {
        if let Some(rest) = strip_prefix_ignore_ascii_case(stem, prefix)
            && let [digit] = rest.as_bytes()
            && digit.is_ascii_digit()
        {
            return true;
        }
    }

    false
}

/// Strips a case-insensitive ASCII prefix, returning the remainder when present.
fn strip_prefix_ignore_ascii_case<'a>(value: &'a str, prefix: &str) -> Option<&'a str> {
    let candidate = value.get(..prefix.len())?;
    candidate
        .eq_ignore_ascii_case(prefix)
        .then(|| value.get(prefix.len()..))
        .flatten()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn keeps_safe_characters() {
        assert_eq!(sanitize_file_name("read_cell"), "read_cell");
        assert_eq!(sanitize_file_name("op-1.v2_final"), "op-1.v2_final");
    }

    #[test]
    fn replaces_path_separators() {
        assert_eq!(sanitize_file_name("group/case"), "group_case");
        assert_eq!(sanitize_file_name("a\\b"), "a_b");
        assert_eq!(sanitize_file_name("../../etc/passwd"), ".._.._etc_passwd");
    }

    #[test]
    fn replaces_whitespace_and_symbols() {
        assert_eq!(sanitize_file_name("a b c"), "a_b_c");
        assert_eq!(sanitize_file_name("a:b*c?"), "a_b_c_");
    }

    #[test]
    fn replaces_non_ascii() {
        assert_eq!(sanitize_file_name("naïve"), "na_ve");
    }

    #[test]
    fn empty_input_yields_underscore() {
        assert_eq!(sanitize_file_name(""), "_");
    }

    #[test]
    fn strips_trailing_dots() {
        assert_eq!(sanitize_file_name("report."), "report");
        assert_eq!(sanitize_file_name("report..."), "report");
    }

    #[test]
    fn dot_and_dotdot_yield_underscore() {
        assert_eq!(sanitize_file_name("."), "_");
        assert_eq!(sanitize_file_name(".."), "_");
    }

    #[test]
    fn escapes_reserved_device_names() {
        assert_eq!(sanitize_file_name("CON"), "_CON");
        assert_eq!(sanitize_file_name("nul"), "_nul");
        assert_eq!(sanitize_file_name("Aux"), "_Aux");
        assert_eq!(sanitize_file_name("PRN"), "_PRN");
    }

    #[test]
    fn escapes_reserved_device_names_with_extension() {
        // Windows treats `CON.json` as the `CON` device, so it must be escaped.
        assert_eq!(sanitize_file_name("CON.json"), "_CON.json");
        assert_eq!(sanitize_file_name("nul.txt"), "_nul.txt");
    }

    #[test]
    fn escapes_reserved_port_names() {
        assert_eq!(sanitize_file_name("COM1"), "_COM1");
        assert_eq!(sanitize_file_name("com9"), "_com9");
        assert_eq!(sanitize_file_name("LPT1"), "_LPT1");
        assert_eq!(sanitize_file_name("lpt0"), "_lpt0");
    }

    #[test]
    fn keeps_names_that_merely_start_like_reserved_names() {
        // Only exact device names (before the extension) are reserved.
        assert_eq!(sanitize_file_name("CONFIG"), "CONFIG");
        assert_eq!(sanitize_file_name("console"), "console");
        assert_eq!(sanitize_file_name("COM"), "COM");
        assert_eq!(sanitize_file_name("COM10"), "COM10");
        assert_eq!(sanitize_file_name("COMA"), "COMA");
    }
}
