use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::PathBuf;

use ohno::AppError;

use crate::publication::binaries::model::{InvalidPlan, identifier};

/// Only workflow-owned plan and execution operations are accepted.
#[derive(Debug)]
pub(crate) enum Cli {
    Plan {
        input: PathBuf,
        repository: String,
    },
    Run {
        input: PathBuf,
        repository: String,
        controller: PathBuf,
        output: PathBuf,
        no_upload: bool,
    },
}

impl Cli {
    pub(crate) fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Self, AppError> {
        let mut arguments = arguments.into_iter();
        let operation = arguments.next().ok_or_else(|| InvalidPlan::new(
            "Expected plan or run; required options: --input, --repository; run also needs --controller and --output".to_owned()
        ))?;
        let mut values = BTreeMap::new();
        let mut no_upload = false;
        while let Some(key) = arguments.next() {
            if key == "--no-upload" && !no_upload {
                no_upload = true;
                continue;
            }
            if !["--input", "--repository", "--controller", "--output"]
                .iter()
                .any(|k| key == *k)
            {
                return Err(InvalidPlan::new(format!("Unknown option: {}", key.display())).into());
            }
            let value = arguments
                .next()
                .ok_or_else(|| InvalidPlan::new(format!("Missing value for {}", key.display())))?;
            if value.is_empty() || values.insert(key.clone(), value).is_some() {
                return Err(InvalidPlan::new(format!(
                    "Empty or repeated option: {}",
                    key.display()
                ))
                .into());
            }
        }
        let input = PathBuf::from(required(&mut values, "--input")?);
        let repository = required(&mut values, "--repository")?
            .into_string()
            .map_err(|value| InvalidPlan::new(format!("Repository must be UTF-8: {value:?}")))?;
        let parts = repository.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || !parts
                .iter()
                .all(|p| identifier(p) || valid_repository_part(p))
        {
            return Err(InvalidPlan::new("Repository must be owner/name".to_owned()).into());
        }
        let cli = if operation == "plan" && !no_upload {
            Self::Plan { input, repository }
        } else if operation == "run" {
            Self::Run {
                input,
                repository,
                controller: PathBuf::from(required(&mut values, "--controller")?),
                output: PathBuf::from(required(&mut values, "--output")?),
                no_upload,
            }
        } else {
            return Err(InvalidPlan::new("Expected plan or run".to_owned()).into());
        };
        if !values.is_empty() {
            return Err(
                InvalidPlan::new("Options do not apply to this operation".to_owned()).into(),
            );
        }
        Ok(cli)
    }
}

fn valid_repository_part(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
}

fn required(values: &mut BTreeMap<OsString, OsString>, key: &str) -> Result<OsString, AppError> {
    values
        .remove(&OsString::from(key))
        .ok_or_else(|| InvalidPlan::new(format!("Missing {key}")).into())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn accepts_internal_commands_and_rejects_ambiguous_options() {
        assert!(matches!(
            Cli::parse(["plan", "--input", "a.json", "--repository", "owner/repo"].map(Into::into))
                .unwrap(),
            Cli::Plan { .. }
        ));
        Cli::parse(["plan", "--input", "a", "--repository", "owner/repo.name"].map(Into::into))
            .unwrap();
        assert!(matches!(
            Cli::parse(
                [
                    "run",
                    "--input",
                    "a.json",
                    "--repository",
                    "owner/repo",
                    "--controller",
                    ".",
                    "--output",
                    "out",
                    "--no-upload"
                ]
                .map(Into::into)
            )
            .unwrap(),
            Cli::Run {
                no_upload: true,
                ..
            }
        ));
        for args in [
            vec![],
            vec!["invalid"],
            vec!["plan", "--unknown"],
            vec!["plan", "--input", "a", "--repository", "owner/.repo"],
            vec!["plan", "--input", "a", "--repository", "owner/a?b"],
            vec!["plan", "--input"],
            vec!["plan", "--input", "a", "--input", "b"],
            vec!["plan", "--input", "a", "--repository", "../bad"],
            vec![
                "plan",
                "--input",
                "a",
                "--repository",
                "owner/repo",
                "--no-upload",
            ],
            vec![
                "plan",
                "--input",
                "a",
                "--repository",
                "owner/repo",
                "--output",
                "bad",
            ],
        ] {
            Cli::parse(args.into_iter().map(Into::into)).unwrap_err();
        }
    }
}
