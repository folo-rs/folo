use std::collections::BTreeMap;
use std::path::PathBuf;

use semver::Version;

/// Binds source verification to exact package versions and a frozen release history.
#[derive(Debug)]
pub struct CandidateRequest {
    pub manifest_path: PathBuf,
    pub commit: String,
    pub release_line: String,
    pub packages: BTreeMap<String, Version>,
    pub verbose: bool,
}

pub(crate) fn package_identifier(name: &str) -> bool {
    // A request identifies a Cargo package, not a path or registry-qualified selector.
    name.bytes()
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_cargo_package_identifiers() {
        for name in ["example-crate", "another_crate", "_internal", "a1"] {
            assert!(package_identifier(name));
        }
        for name in [
            "",
            "../example",
            "name space",
            "1example",
            "-example",
            "a@1.0.0",
        ] {
            assert!(!package_identifier(name));
        }
    }
}
