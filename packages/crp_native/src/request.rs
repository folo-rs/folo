use std::path::Path;
use std::time::Instant;

use crp_workspace::identity::immutable_commit;
use ohno::AppError;
use semver::Version;

/// Validated source and artifact inputs for one native build, without publication policy.
#[derive(Clone, Debug)]
pub struct BuildRequest {
    pub(crate) name: String,
    pub(crate) bin: String,
    pub(crate) version: String,
    pub(crate) label: String,
    pub(crate) source_sha: String,
    pub(crate) archive_base: String,
}

impl BuildRequest {
    pub fn new(
        name: String,
        bin: String,
        version: String,
        label: String,
        source_sha: String,
        archive_base: String,
    ) -> Result<Self, AppError> {
        if !identifier(&name)
            || !identifier(&bin)
            || Version::parse(&version)?.to_string() != version
            || !immutable_commit(&source_sha)
            || archive_base.is_empty()
            || matches!(archive_base.as_str(), "." | "..")
            || !archive_base.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'+')
            })
        {
            return Err(InvalidPlan::new(
                "Invalid native build identity or archive basename".to_owned(),
            )
            .into());
        }
        Ok(Self {
            name,
            bin,
            version,
            label,
            source_sha,
            archive_base,
        })
    }
}

/// Supplies scoped repository-read access when an immutable source object is missing.
pub trait SourceProvider {
    fn fetch(&self, controller: &Path, commit: &str, deadline: Instant) -> Result<(), AppError>;
}

/// Carries the native operation's actual directory and deadline into supervised delivery.
#[derive(Clone, Copy, Debug)]
pub struct ExecutionContext<'a> {
    pub directory: &'a Path,
    pub deadline: Instant,
}

/// The archive pair emitted by native execution, without a publication receipt.
#[derive(Clone, Copy, Debug)]
pub struct Artifacts<'a> {
    pub archive: &'a Path,
    pub checksum: &'a Path,
}

/// Invalid execution inputs or missing stage state prevent native work.
#[ohno::error]
#[display("{message}")]
pub(crate) struct InvalidPlan {
    pub(crate) message: String,
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn binary(name: &str) -> BuildRequest {
        BuildRequest::new(
            name.to_owned(),
            format!("{name}-bin"),
            "1.2.3".to_owned(),
            format!("{name}-v1.2.3"),
            "a".repeat(40),
            format!("{name}-v1.2.3-native"),
        )
        .unwrap()
    }

    #[test]
    fn execution_inputs_require_safe_names_and_complete_source_identity() {
        let request = binary("tool");
        for (name, bin, version, source, archive) in [
            ("../tool", "tool", "1.2.3", "a".repeat(40), "archive"),
            ("tool", "../bin", "1.2.3", "a".repeat(40), "archive"),
            ("tool", "bin", "invalid", "a".repeat(40), "archive"),
            ("tool", "bin", "1.2.3", "short".to_owned(), "archive"),
            ("tool", "bin", "1.2.3", "a".repeat(40), "../archive"),
            ("tool", "bin", "1.2.3", "a".repeat(40), ".."),
            ("tool", "bin", "1.2.3", "a".repeat(40), ""),
        ] {
            BuildRequest::new(
                name.to_owned(),
                bin.to_owned(),
                version.to_owned(),
                request.label.clone(),
                source,
                archive.to_owned(),
            )
            .unwrap_err();
        }
    }
}
