//! Pure parsing of `rustc --verbose --version` (`rustc -vV`) output into the
//! toolchain version and host triple the tool records.

/// The toolchain facts extracted from `rustc -vV` output.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RustcInfo {
    /// The release version (e.g. `1.91.0`), if found.
    pub version: Option<String>,
    /// The host target triple (e.g. `x86_64-unknown-linux-gnu`), if found.
    pub host: Option<String>,
}

/// Parses `rustc -vV` output (pure).
///
/// Reads the `release:` and `host:` lines. If `release:` is absent it falls back
/// to the version token on the leading `rustc <version>` line.
pub(crate) fn parse_rustc_verbose(output: &str) -> RustcInfo {
    let mut info = RustcInfo::default();

    for line in output.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("release:") {
            info.version = non_empty(value.trim());
        } else if let Some(value) = line.strip_prefix("host:") {
            info.host = non_empty(value.trim());
        }
    }

    if info.version.is_none() {
        info.version = output
            .lines()
            .next()
            .and_then(|line| line.trim().strip_prefix("rustc "))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(non_empty);
    }

    info
}

/// Returns `Some(owned)` for a non-empty string, `None` otherwise.
fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_owned())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
rustc 1.91.0 (deadbeef 2025-01-01)
binary: rustc
commit-hash: deadbeefdeadbeefdeadbeefdeadbeefdeadbeef
commit-date: 2025-01-01
host: x86_64-unknown-linux-gnu
release: 1.91.0
LLVM version: 20.1.0
";

    #[test]
    fn parses_release_and_host() {
        let info = parse_rustc_verbose(SAMPLE);
        assert_eq!(info.version.as_deref(), Some("1.91.0"));
        assert_eq!(info.host.as_deref(), Some("x86_64-unknown-linux-gnu"));
    }

    #[test]
    fn falls_back_to_leading_line_version() {
        let output = "rustc 1.90.0 (abc 2024-12-31)\nhost: aarch64-apple-darwin\n";
        let info = parse_rustc_verbose(output);
        assert_eq!(info.version.as_deref(), Some("1.90.0"));
        assert_eq!(info.host.as_deref(), Some("aarch64-apple-darwin"));
    }

    #[test]
    fn missing_host_is_none() {
        let info = parse_rustc_verbose("rustc 1.91.0\nrelease: 1.91.0\n");
        assert_eq!(info.version.as_deref(), Some("1.91.0"));
        assert_eq!(info.host, None);
    }

    #[test]
    fn empty_output_yields_no_facts() {
        assert_eq!(parse_rustc_verbose(""), RustcInfo::default());
    }

    #[test]
    fn empty_release_and_host_values_are_ignored() {
        let info = parse_rustc_verbose("release:\nhost:\n");
        assert_eq!(info, RustcInfo::default());
    }
}
