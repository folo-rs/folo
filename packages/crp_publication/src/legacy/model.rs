use std::collections::BTreeSet;

use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::publication::binaries::Binary;
use crate::publication::binaries::model::{InvalidPlan, identifier};

/// Bootstrap-only input: immutable requests and the caller's native runner table.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Plan {
    pub(crate) targets: Vec<Target>,
    pub(crate) binaries: Vec<Request>,
}

/// The legacy wire protocol attaches target restrictions to the shared binary identity.
#[derive(Debug, Deserialize)]
pub(crate) struct Request {
    #[serde(flatten)]
    pub(crate) binary: Binary,
    /// An empty list selects every configured target; nonempty lists restrict that set.
    pub(crate) release_targets: Vec<String>,
}

/// A runner assignment supplied by the bootstrap workflow, not chosen by native execution.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Target {
    pub(crate) triple: String,
    // The private protocol's `os` key carries the runner label.
    #[serde(rename = "os")]
    pub(crate) runner: String,
}

/// One legacy matrix entry, kept separate from the unified manifest-linked batch envelope.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Batch {
    pub(crate) triple: String,
    #[serde(rename = "os")]
    pub(crate) runner: String,
    pub(crate) timeout_minutes: usize,
    pub(crate) binaries: Vec<Binary>,
}

impl Batch {
    pub(crate) fn validate(&self) -> Result<(), AppError> {
        if !identifier(&self.triple)
            || !runner_label(&self.runner)
            || self.binaries.is_empty()
            || self.timeout_minutes != timeout_minutes(self.binaries.len())
        {
            return Err(InvalidPlan::new("Invalid platform batch".to_owned()).into());
        }
        let mut identities = BTreeSet::new();
        for binary in &self.binaries {
            binary.validate()?;
            if !identities.insert((&binary.name, &binary.version)) {
                return Err(InvalidPlan::new(format!("Duplicate release: {}", binary.tag)).into());
            }
        }
        Ok(())
    }
}

// Cold setup follows .github/workflows/design.md, "Job timeouts". Per-item scaling is a
// conservative hang allowance, capped by the hosted job ceiling, not reserved execution time.
// The workflow and native item deadlines are independent last-chance safeguards.
const SETUP_MINUTES: usize = 90;
const ITEM_MINUTES: usize = 60;
const JOB_MINUTES: usize = 360;

pub(crate) fn timeout_minutes(items: usize) -> usize {
    SETUP_MINUTES
        .saturating_add(ITEM_MINUTES.saturating_mul(items))
        .min(JOB_MINUTES)
}

pub(crate) fn runner_label(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::publication::binaries::model::tests::binary;

    #[test]
    fn legacy_plan_retains_target_restrictions_and_binary_identity() {
        let mut request = serde_json::to_value(binary("tool")).unwrap();
        _ = request
            .as_object_mut()
            .unwrap()
            .insert("release_targets".to_owned(), json!(["native"]));
        let plan: Plan = serde_json::from_value(json!({
            "targets": [{"triple": "native", "os": "runner"}],
            "binaries": [request]
        }))
        .unwrap();
        let request = plan.binaries.first().unwrap();
        assert_eq!(request.binary, binary("tool"));
        assert_eq!(request.release_targets, ["native"]);
    }

    #[test]
    fn timeout_preserves_legacy_allowances_and_caps_large_batches() {
        assert_eq!(timeout_minutes(1), 150);
        assert_eq!(timeout_minutes(3), 270);
        assert_eq!(timeout_minutes(usize::MAX), 360);
    }

    #[test]
    fn invalid_batches_fail_before_native_execution() {
        let valid = Batch {
            triple: "native".into(),
            runner: "runner".into(),
            timeout_minutes: timeout_minutes(1),
            binaries: vec![binary("tool")],
        };
        valid.validate().unwrap();
        for invalid in [
            Batch {
                triple: "../native".into(),
                ..valid.clone()
            },
            Batch {
                runner: "../runner".into(),
                ..valid.clone()
            },
            Batch {
                timeout_minutes: 0,
                ..valid.clone()
            },
            Batch {
                binaries: Vec::new(),
                ..valid.clone()
            },
            Batch {
                binaries: vec![binary("tool"), binary("tool")],
                timeout_minutes: timeout_minutes(2),
                ..valid
            },
        ] {
            invalid.validate().unwrap_err();
        }
    }
}
