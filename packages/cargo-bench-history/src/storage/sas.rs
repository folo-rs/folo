//! Self-signed account shared-access-signature (SAS) tokens for the Azure Blob
//! backend.
//!
//! The shipped Azure SDK exposes no shared-key credential, so to authenticate
//! with a storage account key — the only option the Azurite emulator accepts over
//! plain HTTP — the backend mints an *account SAS* itself: an HMAC-SHA256
//! signature over a canonical string-to-sign, carried in the endpoint URL's query
//! string. The SDK preserves that query across every request, so a single token
//! authorizes the container and blob operations the backend performs.
//!
//! Account SAS is also a first-class production authentication mode: a SAS minted
//! here (or supplied verbatim by the operator) reaches a real storage account
//! over HTTPS exactly as it reaches Azurite over HTTP.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use hmac::{Hmac, KeyInit as _, Mac as _};
use sha2::Sha256;

/// The signed SAS service version. It selects the string-to-sign layout; any
/// value at or after `2020-12-06` uses the trailing signed-encryption-scope
/// field, which this module always emits (empty).
pub(crate) const SAS_VERSION: &str = "2021-08-06";

/// The signed services field for an account SAS scoped to Blob storage.
const SIGNED_SERVICES: &str = "b";

/// Inputs that fully determine an account SAS token.
pub(crate) struct AccountSasParams<'a> {
    /// The storage account name (e.g. `devstoreaccount1` for Azurite).
    pub(crate) account: &'a str,
    /// The base64-encoded storage account key used to sign the token.
    pub(crate) account_key_base64: &'a str,
    /// The signed permissions (e.g. `rwdlac`).
    pub(crate) permissions: &'a str,
    /// The signed resource types (e.g. `sco` = service, container, object).
    pub(crate) resource_types: &'a str,
    /// The signed expiry, formatted `YYYY-MM-DDThh:mm:ssZ`.
    pub(crate) expiry: &'a str,
    /// The signed protocol (`https` for real Azure, `https,http` for Azurite).
    pub(crate) protocol: &'a str,
}

/// A failure to mint an account SAS token.
#[derive(Debug)]
pub(crate) enum SasError {
    /// The configured account key was not valid base64.
    InvalidAccountKey(base64::DecodeError),
}

impl std::fmt::Display for SasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidAccountKey(_) => f.write_str("account key is not valid base64"),
        }
    }
}

impl std::error::Error for SasError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidAccountKey(error) => Some(error),
        }
    }
}

/// Builds the canonical account-SAS string-to-sign.
///
/// The ten fields are joined with `\n` and the whole string ends with a trailing
/// `\n`. `signedStart`, `signedIP`, and `signedEncryptionScope` are always empty.
fn string_to_sign(params: &AccountSasParams<'_>) -> String {
    format!(
        "{account}\n{permissions}\n{services}\n{resource_types}\n\n{expiry}\n\n{protocol}\n{version}\n\n",
        account = params.account,
        permissions = params.permissions,
        services = SIGNED_SERVICES,
        resource_types = params.resource_types,
        expiry = params.expiry,
        protocol = params.protocol,
        version = SAS_VERSION,
    )
}

/// Computes the base64 HMAC-SHA256 signature for `params`.
fn signature(params: &AccountSasParams<'_>) -> Result<String, SasError> {
    let key = BASE64
        .decode(params.account_key_base64)
        .map_err(SasError::InvalidAccountKey)?;
    // HMAC accepts a key of any length, so `new_from_slice` cannot fail here.
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).expect("HMAC accepts a key of any length");
    mac.update(string_to_sign(params).as_bytes());
    Ok(BASE64.encode(mac.finalize().into_bytes()))
}

/// Mints an account SAS and returns it as a URL query string (without a leading
/// `?`), with every value percent-encoded for direct use as a URL query.
///
/// # Errors
///
/// Returns [`SasError::InvalidAccountKey`] if the configured account key is not
/// valid base64 and therefore cannot be used to sign the token.
pub(crate) fn account_sas_query(params: &AccountSasParams<'_>) -> Result<String, SasError> {
    let signature = signature(params)?;
    let pairs = [
        ("sv", SAS_VERSION),
        ("ss", SIGNED_SERVICES),
        ("srt", params.resource_types),
        ("sp", params.permissions),
        ("se", params.expiry),
        ("spr", params.protocol),
        ("sig", signature.as_str()),
    ];
    Ok(encode_query(&pairs))
}

/// Form-encodes ordered key/value pairs into a query string. Reserved characters
/// in values (e.g. `:` in the expiry, `,` in the protocol, `+`/`/`/`=` in the
/// base64 signature) are percent-encoded so the query parses unambiguously.
fn encode_query(pairs: &[(&str, &str)]) -> String {
    let mut url = azure_core::http::Url::parse("http://sas.invalid/")
        .expect("the constant base URL is valid");
    url.query_pairs_mut().extend_pairs(pairs.iter().copied());
    url.query().expect("a query was just appended").to_owned()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    /// The well-known Azurite development account key. Public, fixed, and used
    /// only against the local emulator — not a secret.
    const AZURITE_KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    fn golden_params() -> AccountSasParams<'static> {
        AccountSasParams {
            account: "devstoreaccount1",
            account_key_base64: AZURITE_KEY,
            permissions: "rwdlac",
            resource_types: "sco",
            expiry: "2030-01-01T00:00:00Z",
            protocol: "https,http",
        }
    }

    #[test]
    fn string_to_sign_has_ten_newline_terminated_fields() {
        let sts = string_to_sign(&golden_params());
        assert_eq!(
            sts,
            "devstoreaccount1\nrwdlac\nb\nsco\n\n2030-01-01T00:00:00Z\n\nhttps,http\n2021-08-06\n\n"
        );
        // Ten fields, each terminated by `\n`.
        assert_eq!(sts.matches('\n').count(), 10);
    }

    #[test]
    fn signature_matches_independent_golden_vector() {
        // Computed independently (HMAC-SHA256 over the canonical string-to-sign);
        // any drift in the canonicalization breaks this exact equality.
        let sig = signature(&golden_params()).unwrap();
        assert_eq!(sig, "1Mh1LSBeLSI4EKTerU15bv2Ml2MomydA5vFxFzkTOyw=");
    }

    #[test]
    fn invalid_account_key_is_rejected() {
        let params = AccountSasParams {
            account_key_base64: "not valid base64!!!",
            ..golden_params()
        };
        let error = signature(&params).unwrap_err();
        assert!(matches!(error, SasError::InvalidAccountKey(_)), "{error}");
        account_sas_query(&params).unwrap_err();
    }

    #[test]
    fn query_carries_every_signed_field_and_percent_encodes_values() {
        let query = account_sas_query(&golden_params()).unwrap();
        assert!(query.contains("sv=2021-08-06"), "{query}");
        assert!(query.contains("ss=b"), "{query}");
        assert!(query.contains("srt=sco"), "{query}");
        assert!(query.contains("sp=rwdlac"), "{query}");
        // The expiry's colons and the protocol's comma are percent-encoded.
        assert!(query.contains("se=2030-01-01T00%3A00%3A00Z"), "{query}");
        assert!(query.contains("spr=https%2Chttp"), "{query}");
        // The signature appears percent-encoded (its `=` padding becomes `%3D`).
        assert!(
            query.contains("sig=1Mh1LSBeLSI4EKTerU15bv2Ml2MomydA5vFxFzkTOyw%3D"),
            "{query}"
        );
    }

    #[test]
    fn sas_error_display_is_human_readable() {
        let error = signature(&AccountSasParams {
            account_key_base64: "not valid base64!!!",
            ..golden_params()
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "account key is not valid base64");
    }

    #[test]
    fn sas_error_exposes_the_underlying_base64_error_as_source() {
        use std::error::Error as _;

        let error = signature(&AccountSasParams {
            account_key_base64: "not valid base64!!!",
            ..golden_params()
        })
        .unwrap_err();
        let source = error
            .source()
            .expect("InvalidAccountKey wraps the underlying base64 decode error");
        assert!(source.downcast_ref::<base64::DecodeError>().is_some());
    }
}
