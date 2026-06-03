// Version requirement freezing logic.
//
// Given a version requirement string in Cargo's `semver`-flavored syntax (e.g. `"1.2.3"`,
// `"^1.2"`, `">=1.2.3"`), produce a literal `=major.minor.patch[-pre]` string. If the
// requirement is anything other than a single freezable comparator, return `None` to
// indicate the requirement should be left unchanged.

use semver::{BuildMetadata, Comparator, Op, Version, VersionReq};

/// Computes the frozen `=X.Y.Z[-pre]` string for the given version requirement.
///
/// Returns `Ok(None)` if the requirement has no freezable form and the caller should
/// leave the original text unchanged.
///
/// # Errors
///
/// Returns the underlying `semver::Error` if the input is not a valid version requirement
/// per Cargo's flavor of `SemVer`.
///
/// # Scope
///
/// Only requirements consisting of a single comparator with an "equals" component
/// (`=`, `^`, `~`, `>=`, `<=`, or a wildcard with a major number) are freezable. Anything
/// else — strict inequalities, bare `*`, or multi-comparator requirements like
/// `">=0.5, <1.0"` — is left unchanged.
pub(crate) fn freeze_requirement(input: &str) -> Result<Option<String>, semver::Error> {
    let req: VersionReq = input.parse()?;

    // Multi-comparator requirements (e.g. `">=0.5, <1.0"`) are intentionally out of scope:
    // picking a single literal that preserves the original constraint would require either
    // ignoring some of the comparators (non-preserving) or knowing which versions actually
    // exist on the registry (out of reach for this tool).
    let [comparator] = req.comparators.as_slice() else {
        return Ok(None);
    };

    Ok(equals_version(comparator).as_ref().map(format_frozen))
}

/// If `c` carries an "equals" component (a specific version that satisfies it), returns the
/// corresponding fully-expanded `Version`. Otherwise returns `None`.
fn equals_version(c: &Comparator) -> Option<Version> {
    match c.op {
        // All of these allow the specified version itself as a match. We zero-fill missing
        // minor/patch numbers per the user's specification.
        Op::Exact | Op::Caret | Op::Tilde | Op::GreaterEq | Op::LessEq | Op::Wildcard => {
            Some(Version {
                major: c.major,
                minor: c.minor.unwrap_or(0),
                patch: c.patch.unwrap_or(0),
                pre: c.pre.clone(),
                // Build metadata is never present on a `Comparator` (semver strips it from
                // version requirements per the spec), so we always emit empty here.
                build: BuildMetadata::EMPTY,
            })
        }
        // `Op::Greater` and `Op::Less` carry no equals component. `Op` is also
        // `#[non_exhaustive]`, so we catch any future unknown operators with a wildcard
        // and skip them conservatively rather than risk freezing them to a wrong value.
        _ => None,
    }
}

fn format_frozen(v: &Version) -> String {
    if v.pre.is_empty() {
        format!("={}.{}.{}", v.major, v.minor, v.patch)
    } else {
        format!("={}.{}.{}-{}", v.major, v.minor, v.patch, v.pre)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn freeze(input: &str) -> Option<String> {
        freeze_requirement(input).unwrap()
    }

    // -- Basic three-component versions and defaults --------------------------------------

    #[test]
    fn three_components_default() {
        assert_eq!(freeze("1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn single_component_zero_fills() {
        assert_eq!(freeze("1").as_deref(), Some("=1.0.0"));
    }

    #[test]
    fn two_components_zero_fills() {
        assert_eq!(freeze("1.2").as_deref(), Some("=1.2.0"));
    }

    #[test]
    fn zero_major_three_components() {
        assert_eq!(freeze("0.5.10").as_deref(), Some("=0.5.10"));
    }

    // -- Exact (= operator) ---------------------------------------------------------------

    #[test]
    fn exact_three_components() {
        assert_eq!(freeze("=1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn exact_two_components_zero_fills() {
        assert_eq!(freeze("=1.2").as_deref(), Some("=1.2.0"));
    }

    #[test]
    fn exact_one_component_zero_fills() {
        assert_eq!(freeze("=1").as_deref(), Some("=1.0.0"));
    }

    #[test]
    fn idempotent_already_frozen() {
        // Freezing an already-frozen value yields the same text.
        let frozen = freeze("=1.2.3").unwrap();
        assert_eq!(freeze(&frozen).as_deref(), Some("=1.2.3"));
    }

    // -- Caret (^) and tilde (~) ----------------------------------------------------------

    #[test]
    fn caret_three_components() {
        assert_eq!(freeze("^1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn caret_two_components() {
        assert_eq!(freeze("^1.2").as_deref(), Some("=1.2.0"));
    }

    #[test]
    fn tilde_three_components() {
        assert_eq!(freeze("~1.2.3").as_deref(), Some("=1.2.3"));
    }

    // -- GreaterEq and LessEq -------------------------------------------------------------

    #[test]
    fn greater_eq_three_components() {
        assert_eq!(freeze(">=1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn greater_eq_with_space() {
        assert_eq!(freeze(">= 1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn greater_eq_one_component() {
        assert_eq!(freeze(">=1").as_deref(), Some("=1.0.0"));
    }

    #[test]
    fn less_eq_three_components() {
        assert_eq!(freeze("<=1.2.3").as_deref(), Some("=1.2.3"));
    }

    #[test]
    fn less_eq_two_components() {
        assert_eq!(freeze("<=1.2").as_deref(), Some("=1.2.0"));
    }

    #[test]
    fn less_eq_one_component() {
        assert_eq!(freeze("<=1").as_deref(), Some("=1.0.0"));
    }

    // -- Wildcards ------------------------------------------------------------------------

    #[test]
    fn wildcard_with_major() {
        assert_eq!(freeze("1.*").as_deref(), Some("=1.0.0"));
    }

    #[test]
    fn wildcard_with_major_and_minor() {
        assert_eq!(freeze("1.2.*").as_deref(), Some("=1.2.0"));
    }

    #[test]
    fn wildcard_triple_star_form() {
        // Semver documents `1.*.*` as equivalent to `1.*`.
        assert_eq!(freeze("1.*.*").as_deref(), Some("=1.0.0"));
    }

    #[test]
    fn bare_wildcard_left_unchanged() {
        // Bare `*` parses to an empty comparator list; there is no concrete version to pick.
        assert_eq!(freeze("*"), None);
    }

    // -- Strict inequalities (no equals component) ----------------------------------------

    #[test]
    fn strict_less_unchanged() {
        assert_eq!(freeze("<1.2.3"), None);
    }

    #[test]
    fn strict_greater_unchanged() {
        assert_eq!(freeze(">1.2.3"), None);
    }

    // -- Multi-comparator requirements are out of scope -----------------------------------

    #[test]
    fn multi_comparator_left_unchanged_even_when_all_freezable() {
        // Two `=` comparators — both individually freezable — but a multi-comparator
        // requirement is out of scope and is left unchanged.
        assert_eq!(freeze("=1.2.3, =1.2.4"), None);
    }

    #[test]
    fn multi_comparator_mixed_left_unchanged() {
        assert_eq!(freeze(">=1.0.0, <2.0.0"), None);
    }

    #[test]
    fn multi_comparator_all_strict_left_unchanged() {
        assert_eq!(freeze(">1.0.0, <2.0.0"), None);
    }

    // -- Pre-release and build metadata ---------------------------------------------------

    #[test]
    fn prerelease_three_components_preserved() {
        assert_eq!(
            freeze("1.2.3-foobar45-wakawaka").as_deref(),
            Some("=1.2.3-foobar45-wakawaka")
        );
    }

    #[test]
    fn prerelease_short_form_is_invalid() {
        // Per the SemVer spec a pre-release suffix is only allowed on a complete
        // major.minor.patch triple. `1.2-foo` is not valid.
        freeze_requirement("1.2-foo").unwrap_err();
    }

    #[test]
    fn build_metadata_silently_dropped() {
        // Per the semver crate docs, build metadata is stripped from version requirements.
        // The frozen output cannot carry build metadata.
        assert_eq!(freeze("1.2.3+build.42").as_deref(), Some("=1.2.3"));
    }

    // -- Whitespace tolerance -------------------------------------------------------------

    #[test]
    fn surrounding_whitespace_tolerated() {
        assert_eq!(freeze("  ^1.2.3  ").as_deref(), Some("=1.2.3"));
    }

    // -- Parse errors ---------------------------------------------------------------------

    #[test]
    fn garbage_input_returns_error() {
        freeze_requirement("not-a-version").unwrap_err();
    }

    #[test]
    fn empty_string_returns_error() {
        freeze_requirement("").unwrap_err();
    }
}
