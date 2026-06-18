//! Sanitizing arbitrary strings into filesystem-safe file name components.

/// Converts an arbitrary string into a filesystem-safe file name component.
///
/// Every character that is not an ASCII alphanumeric or one of `.`, `_`, `-`
/// is replaced with `_`. An empty input produces `_`. The result never
/// contains path separators, so it is always a single path component.
#[must_use]
pub fn sanitize_file_name(name: &str) -> String {
    let sanitized: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();

    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
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
}
