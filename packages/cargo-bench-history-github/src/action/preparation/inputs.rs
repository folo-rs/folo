use std::collections::{BTreeMap, BTreeSet};

use ohno::AppError;

use crate::action::errors::InvalidInput;
use crate::action::inputs::InputObject;
use crate::result::platform_list;

/// Checked workflow inputs; scope policy is not a root collection command setting.
pub(crate) struct WorkflowInputs {
    values: BTreeMap<String, String>,
    pub(crate) excluded: BTreeSet<String>,
}

impl WorkflowInputs {
    /// Validates the entire setup selection before config, Git or package discovery runs.
    pub(crate) fn parse(json: &[u8]) -> Result<Self, AppError> {
        let InputObject(mut values) = serde_json::from_slice(json).map_err(|error| {
            InvalidInput::caused_by("inputs-file", "expected a string object", error)
        })?;
        for (key, value) in &values {
            if !matches!(
                key.as_str(),
                "working-directory" | "config" | "platforms" | "exclude"
            ) {
                return Err(InvalidInput::new(key, "unknown workflow input").into());
            }
            if !value.is_empty() && (value.trim().is_empty() || value.contains(['\r', '\n', '\0']))
            {
                return Err(InvalidInput::new(key, "expected a nonblank single-line value").into());
            }
        }
        values.retain(|_, value| !value.is_empty());
        platform_list(values.get("platforms").ok_or_else(|| {
            InvalidInput::new("platforms", "expected collection platforms are required")
        })?)?;
        let excluded = values
            .get("exclude")
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .map(|name| {
                        if !package_name(name) {
                            return Err(
                                InvalidInput::new("exclude", "expected package names").into()
                            );
                        }
                        Ok(name.to_owned())
                    })
                    .collect::<Result<BTreeSet<_>, AppError>>()
            })
            .transpose()?
            .unwrap_or_default();
        Ok(Self { values, excluded })
    }

    /// Returns the selected path or normalized input source without applying global defaults.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.values.get(name).map(String::as_str)
    }

    pub(crate) fn platforms(&self) -> &str {
        self.get("platforms")
            .expect("workflow input validation requires platforms")
    }
}

/// Keeps concrete package names safe as CSV and repeated Cargo package arguments.
pub(crate) fn package_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|value| value.is_whitespace() || value.is_control() || value == ',')
}
