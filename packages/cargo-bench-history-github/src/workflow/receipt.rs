use std::num::NonZero;
use std::panic::{RefUnwindSafe, UnwindSafe};

use ohno::AppError;
use serde::{Deserialize, Serialize};

use crate::model::{CommitSha, Instance, Repository};
use crate::result::is_platform_identifier;

/// Validated run identity and measured hardware, independent of artifact paths.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Receipt {
    pub(crate) repository: Repository,
    pub(crate) instance: Instance,
    pub(crate) run_id: NonZero<u64>,
    pub(crate) run_attempt: NonZero<u64>,
    pub(crate) head: CommitSha,
    pub(crate) platform: String,
    pub(crate) machine_key: String,
}

impl Receipt {
    /// Decodes artifact evidence before it can participate in job-attempt reconciliation.
    pub(crate) fn parse(json: &[u8]) -> Result<Self, AppError> {
        let raw: ReceiptWire = serde_json::from_slice(json).map_err(InvalidReceipt::caused_by)?;
        if raw.version != RECEIPT_VERSION {
            return Err(InvalidReceipt::new().into());
        }
        validate_platform(&raw.platform)?;
        Ok(Self {
            repository: raw.repository.parse()?,
            instance: raw.instance.parse()?,
            run_id: raw.run_id,
            run_attempt: raw.run_attempt,
            head: raw.head.parse()?,
            platform: raw.platform,
            machine_key: machine_key(&raw.machine_key)?,
        })
    }

    /// Serializes validated collection identity for the receipt-only artifact handoff.
    pub(crate) fn encode(&self) -> Result<Vec<u8>, AppError> {
        serde_json::to_vec(&ReceiptWire {
            version: RECEIPT_VERSION,
            repository: self.repository.to_string(),
            instance: self.instance.as_str().to_owned(),
            run_id: self.run_id,
            run_attempt: self.run_attempt,
            head: self.head.as_str().to_owned(),
            platform: self.platform.clone(),
            machine_key: self.machine_key.clone(),
        })
        .map_err(|error| InvalidReceipt::caused_by(error).into())
    }
}

// This versions only the companion's collection record, never the analyzer's report.
const RECEIPT_VERSION: u32 = 1;

/// Internal artifact representation decoded before any job reconciliation.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ReceiptWire {
    version: u32,
    repository: String,
    instance: String,
    run_id: NonZero<u64>,
    run_attempt: NonZero<u64>,
    head: String,
    platform: String,
    machine_key: String,
}

/// Validates and normalizes a captured core fingerprint for receipts and analyzer filters.
pub(crate) fn machine_key(value: &str) -> Result<String, AppError> {
    // The machine-key command prints the core tool's 64-bit hexadecimal fingerprint.
    const KEY_DIGITS: usize = 16;
    if value.len() != KEY_DIGITS || !value.bytes().all(|one| one.is_ascii_hexdigit()) {
        return Err(InvalidMachineKey::new().into());
    }
    Ok(value.to_ascii_lowercase())
}

/// Applies the shared identifier rule before a platform names a job or artifact directory.
pub(crate) fn validate_platform(value: &str) -> Result<(), AppError> {
    if !is_platform_identifier(value) {
        return Err(InvalidCollectionPlatform::new().into());
    }
    Ok(())
}

/// A receipt cannot establish collection identity until its entire record is valid.
#[ohno::error]
#[display("Collection receipt has malformed or unsupported metadata")]
pub(crate) struct InvalidReceipt;

/// Machine selection must come from a genuine machine-key fingerprint.
#[ohno::error]
#[display("Machine key must contain exactly 16 hexadecimal digits")]
pub(crate) struct InvalidMachineKey;

/// Platform identifiers also name machine-key files, not arbitrary relative paths.
#[ohno::error]
#[display("Collection platform must be a nonempty ASCII matrix identifier, not a path")]
pub(crate) struct InvalidCollectionPlatform;

impl UnwindSafe for InvalidReceipt {}
impl RefUnwindSafe for InvalidReceipt {}
impl UnwindSafe for InvalidMachineKey {}
impl RefUnwindSafe for InvalidMachineKey {}
impl UnwindSafe for InvalidCollectionPlatform {}
impl RefUnwindSafe for InvalidCollectionPlatform {}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub(crate) mod tests {
    use std::collections::BTreeSet;

    use serde_json::{Value, json};

    use super::*;
    use crate::result::platform_list;

    pub(crate) fn receipt(platform: &str, attempt: u64) -> Receipt {
        Receipt {
            repository: "folo-rs/folo".parse().unwrap(),
            instance: "folo".parse().unwrap(),
            run_id: NonZero::new(42).unwrap(),
            run_attempt: NonZero::new(attempt).unwrap(),
            head: "a".repeat(40).parse().unwrap(),
            platform: platform.to_owned(),
            machine_key: "0123456789abcdef".to_owned(),
        }
    }

    #[test]
    fn round_trip_preserves_real_identity() {
        let receipt = receipt("linux", 2);
        assert_eq!(Receipt::parse(&receipt.encode().unwrap()).unwrap(), receipt);
        assert_eq!(
            machine_key("0123456789ABCDEF").unwrap(),
            receipt.machine_key
        );
    }

    #[test]
    fn receipt_requires_known_version_and_valid_fields() {
        let raw: Value = serde_json::from_slice(&receipt("linux", 1).encode().unwrap()).unwrap();
        for (field, value) in [
            ("version", json!(2)),
            ("repository", json!("bad")),
            ("instance", json!("invalid/path")),
            ("run_id", json!(0)),
            ("run_attempt", json!(0)),
            ("head", json!("a".repeat(40) + "-dirty")),
            ("platform", json!("..")),
            ("machine_key", json!("key")),
            ("extra", json!(true)),
        ] {
            let mut invalid = raw.clone();
            invalid
                .as_object_mut()
                .unwrap()
                .insert(field.to_owned(), value);
            Receipt::parse(&serde_json::to_vec(&invalid).unwrap()).unwrap_err();
        }
    }

    #[test]
    fn keys_and_platforms_cannot_inject_outputs_or_paths() {
        for key in [
            "",
            "0123456789abcdeg",
            "0123456789abcdef\n",
            "0123456789abcde",
        ] {
            let error = machine_key(key).unwrap_err();
            assert!(error.find_source::<InvalidMachineKey>().is_some());
        }
        for platform in ["", ".", "..", "a/b", "a\\b", "a\nb", "a,b", " a"] {
            let error = validate_platform(platform).unwrap_err();
            assert!(error.find_source::<InvalidCollectionPlatform>().is_some());
        }
        assert_eq!(
            platform_list("windows, linux,linux").unwrap(),
            BTreeSet::from(["linux".to_owned(), "windows".to_owned()])
        );
    }
}
