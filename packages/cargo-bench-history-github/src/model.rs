use std::fmt;
use std::str::FromStr;

use ohno::AppError;

use crate::errors::{InvalidCommitShaError, InvalidInstanceError, InvalidRepositoryError};

/// Identifies one repository for scoped REST requests and lifecycle discovery.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Repository {
    owner: String,
    name: String,
}

impl Repository {
    pub(crate) fn owner(&self) -> &str {
        &self.owner
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }
}

impl FromStr for Repository {
    type Err = AppError;

    /// Validates literal owner/name URL segments before they enter repository-scoped operations.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((owner, name)) = value.split_once('/') else {
            return Err(InvalidRepositoryError::new(value).into());
        };
        let valid_part = |part: &str| {
            // Dot components would be normalized away when constructing REST URLs.
            !part.is_empty()
                && !matches!(part, "." | "..")
                && part
                    .bytes()
                    .all(|one| one.is_ascii_alphanumeric() || matches!(one, b'.' | b'-' | b'_'))
        };
        if !valid_part(owner) || !valid_part(name) {
            return Err(InvalidRepositoryError::new(value).into());
        }
        Ok(Self {
            owner: owner.to_owned(),
            name: name.to_owned(),
        })
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

/// The project namespace separating report and workflow identities within a repository.
///
/// Action setup resolves storage identity through the core helpers; this type validates its
/// marker-safe representation without defining another project-normalization rule.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct Instance(String);

impl Instance {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for Instance {
    type Err = AppError;

    /// Accepts an internally resolved namespace for use in body markers and job identities.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.is_empty()
            || !value
                .bytes()
                .all(|one| one.is_ascii_alphanumeric() || matches!(one, b'.' | b'-' | b'_'))
        {
            return Err(InvalidInstanceError::new().into());
        }
        Ok(Self(value.to_owned()))
    }
}

/// A full commit identity binding reports, collection receipts and ownership guards.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct CommitSha(String);

impl CommitSha {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for CommitSha {
    type Err = AppError;

    /// Normalizes full hexadecimal identities without resolving a ref or guessing a commit.
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        const SHA_DIGITS: usize = 40;

        if value.len() != SHA_DIGITS || !value.bytes().all(|one| one.is_ascii_hexdigit()) {
            return Err(InvalidCommitShaError::new(value).into());
        }
        Ok(Self(value.to_ascii_lowercase()))
    }
}

/// Independent regression-report and one-off workflow-failure issue kinds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IssueKind {
    Regression,
    FailureAlert,
}

impl IssueKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Regression => "regression",
            Self::FailureAlert => "failure-alert",
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::panic::{RefUnwindSafe, UnwindSafe};

    use static_assertions::assert_impl_all;

    use super::*;

    assert_impl_all!(Repository: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(Instance: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);
    assert_impl_all!(CommitSha: Send, Sync, Unpin, UnwindSafe, RefUnwindSafe);

    #[test]
    fn repository_requires_exactly_owner_and_name() {
        let repository = "folo-rs/folo".parse::<Repository>().unwrap();
        assert_eq!(repository.owner(), "folo-rs");
        assert_eq!(repository.name(), "folo");
        for invalid in ["folo", "/folo", "folo-rs/", "a/b/c"] {
            assert!(invalid.parse::<Repository>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn repository_rejects_url_dot_segments() {
        for invalid in ["./issues", "../issues", "folo-rs/.", "folo-rs/.."] {
            assert!(invalid.parse::<Repository>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn repository_preserves_dots_within_literal_names() {
        for valid in ["folo-rs/.github", "folo-rs/repo..name"] {
            let repository = valid.parse::<Repository>().unwrap();
            assert_eq!(repository.to_string(), valid);
        }
    }

    #[test]
    fn instance_is_safe_inside_an_html_comment() {
        "folo.default".parse::<Instance>().unwrap();
        for invalid in ["", "a b", "a/b", "a-->b"] {
            assert!(invalid.parse::<Instance>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn commit_sha_requires_full_hex() {
        "0123456789abcdef0123456789abcdef01234567"
            .parse::<CommitSha>()
            .unwrap();
        for invalid in ["abc", "g123456789abcdef0123456789abcdef01234567"] {
            assert!(invalid.parse::<CommitSha>().is_err(), "{invalid}");
        }
    }
}
