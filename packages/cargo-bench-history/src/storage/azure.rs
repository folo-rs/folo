//! [`AzureBlobStorage`]: a [`Storage`] backed by an Azure Blob Storage container.
//!
//! Object keys map directly to blob names, so the key model is identical to
//! [`LocalStorage`](super::local::LocalStorage): write-once objects and
//! list-by-prefix, with no special-casing by callers. Hierarchical keys become
//! real `/`-separated blob names (the blob URL is built segment by segment so the
//! separators are never percent-encoded), which makes prefix listing line up with
//! the partition layout.
//!
//! Authentication is resolved once at construction, in priority order: a
//! self-signed account SAS (`account_key`), a verbatim SAS token (`sas_token`),
//! or Microsoft Entra ID. SAS modes carry their token in the endpoint URL's query
//! and pass no credential (so the emulator's plain-HTTP endpoint is accepted);
//! Entra mode passes a token credential and requires HTTPS.

use std::collections::HashMap;
use std::fmt;
use std::io;
use std::sync::Arc;

use azure_core::credentials::{AccessToken, TokenCredential, TokenRequestOptions};
use azure_core::error::ErrorKind;
use azure_core::http::{
    ClientOptions, HttpClient, HttpClientOptions, RequestContent, StatusCode, Transport, Url,
    new_http_client,
};
use azure_core::time::{Duration, OffsetDateTime};
use azure_identity::DeveloperToolsCredential;
use azure_storage_blob::models::{
    BlobClientUploadOptions, BlobContainerClientListBlobsOptions, StorageErrorCode,
};
use azure_storage_blob::{
    BlobClient, BlobClientOptions, BlobContainerClient, BlobContainerClientOptions,
};
use cargo_bench_history_core::codec;
use futures::TryStreamExt as _;
use jiff::tz::TimeZone;
use jiff::{Timestamp, ToSpan as _};

use super::sas::{AccountSasParams, account_sas_query};
use super::{Storage, StorageError, github_oidc, validate_key};

/// The SAS permissions a self-signed token grants: read, write, delete, list,
/// add, create — everything the backend needs to create the container on demand
/// and write, read, and enumerate objects.
const SAS_PERMISSIONS: &str = "rwdlac";

/// The SAS resource types a self-signed token covers: service, container, object.
const SAS_RESOURCE_TYPES: &str = "sco";

/// The HTTP content coding declared on every uploaded blob. The storage layer
/// always stores gzip, so this header is unconditionally truthful and lets a
/// non-SDK reader inflate the blob with standard tooling (the backend still
/// inflates on [`get`](AzureBlobStorage::get) itself rather than relying on the
/// service to decode).
const GZIP_CONTENT_ENCODING: &str = "gzip";

/// A [`Storage`] that persists objects as blobs in an Azure Blob container.
#[derive(Clone)]
pub(crate) struct AzureBlobStorage {
    /// The container endpoint URL. For SAS authentication the signed query is
    /// already applied here and preserved across every derived blob URL.
    container_endpoint: Url,
    /// The token credential for Entra ID authentication, or `None` when a SAS
    /// query on `container_endpoint` carries the authentication instead.
    credential: Option<Arc<dyn TokenCredential>>,
    /// One pooled HTTP client shared by every per-object blob and container
    /// client (injected via the transport seam in [`shared_client_options`]).
    /// All operations then reuse a single `reqwest` connection pool, so the
    /// backend keeps TCP+TLS connections alive across objects instead of paying
    /// a fresh handshake per object (and exhausting ephemeral ports at high
    /// fetch concurrency). Built once in [`from_config`].
    ///
    /// [`shared_client_options`]: AzureBlobStorage::shared_client_options
    /// [`from_config`]: AzureBlobStorage::from_config
    http_client: Arc<dyn HttpClient>,
}

impl fmt::Debug for AzureBlobStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Redact the query string: for SAS authentication it contains the
        // signature, which must never reach logs.
        let mut endpoint = self.container_endpoint.clone();
        endpoint.set_query(None);
        // `http_client` is an opaque shared transport with no meaningful debug
        // representation and no security relevance; omit it explicitly.
        f.debug_struct("AzureBlobStorage")
            .field("endpoint", &endpoint.as_str())
            .field("entra", &self.credential.is_some())
            .finish_non_exhaustive()
    }
}

impl AzureBlobStorage {
    /// Builds an Azure backend from its configured parameters.
    ///
    /// # Errors
    ///
    /// Returns [`StorageError::Config`] if both `account_key` and `sas_token` are
    /// set, if the endpoint is not a valid base URL, if Entra authentication is
    /// selected without an HTTPS endpoint, or if an account SAS cannot be signed.
    pub(crate) fn from_config(
        account: &str,
        container: &str,
        endpoint: Option<String>,
        account_key: Option<String>,
        sas_token: Option<String>,
    ) -> Result<Self, StorageError> {
        if account_key.is_some() && sas_token.is_some() {
            return Err(config_error(
                "set only one of `account_key` or `sas_token` for Azure storage",
            ));
        }

        let endpoint =
            endpoint.unwrap_or_else(|| format!("https://{account}.blob.core.windows.net"));
        let mut container_endpoint = Url::parse(&endpoint).map_err(|error| {
            config_error(format!("invalid Azure endpoint {endpoint:?}: {error}"))
        })?;
        container_endpoint
            .path_segments_mut()
            .map_err(|()| {
                config_error(format!("Azure endpoint {endpoint:?} cannot be a base URL"))
            })?
            .pop_if_empty()
            .push(container);

        // Reject Entra-over-HTTP before building anything else, so this validation
        // error never depends on first constructing the HTTP client (which would pull
        // in `reqwest` and make the check un-runnable under Miri). The SAS modes set a
        // credential-bearing query and so accept the emulator's plain-HTTP endpoint;
        // only the Entra path (neither key nor token) demands HTTPS.
        let is_entra = account_key.is_none() && sas_token.is_none();
        if is_entra && container_endpoint.scheme() != "https" {
            return Err(config_error(
                "Azure Entra ID authentication requires an https endpoint",
            ));
        }

        // Build one HTTP client up front and share it across every per-object blob
        // and container client (via the transport seam in `shared_client_options`),
        // so all operations reuse a single connection pool and keep TCP+TLS
        // connections alive instead of paying a fresh handshake per object.
        // `automatic_decompression` is left off to match the SDK's own per-client
        // default: the storage layer stores gzip and inflates it itself in `get`, so
        // the transport must hand back the raw compressed bytes (turning
        // auto-decompression on would double-inflate). The Entra OIDC credential
        // reuses the same client for its token `GET`; that endpoint is never gzipped
        // (the request advertises no `Accept-Encoding`), so decompression staying off
        // is correct there too.
        let http_client = new_http_client(Some(HttpClientOptions {
            automatic_decompression: false,
        }));

        let credential = if let Some(account_key) = account_key {
            let query = account_sas_query(&AccountSasParams {
                account,
                account_key_base64: &account_key,
                permissions: SAS_PERMISSIONS,
                resource_types: SAS_RESOURCE_TYPES,
                expiry: &account_sas_expiry(),
                protocol: sas_protocol(&container_endpoint),
            })
            .map_err(|error| config_error(format!("could not sign account SAS: {error}")))?;
            container_endpoint.set_query(Some(&query));
            None
        } else if let Some(sas_token) = sas_token {
            container_endpoint.set_query(Some(sas_token.trim_start_matches('?')));
            None
        } else {
            // The endpoint was already confirmed to be HTTPS above.
            let credential = entra_credential(&http_client)?;
            // Wrap the credential so a burst of concurrent reads shares one token
            // instead of each driving its own token acquisition (see
            // `CachingCredential`).
            let caching: Arc<dyn TokenCredential> = Arc::new(CachingCredential::new(credential));
            Some(caching)
        };

        Ok(Self {
            container_endpoint,
            credential,
            http_client,
        })
    }

    /// The client options every per-object client is built with: a transport
    /// backed by the one shared, pooled [`http_client`](Self::http_client), so
    /// all operations reuse a single `reqwest` connection pool (keep-alive)
    /// rather than each opening its own and handshaking afresh.
    // Mutation-skipped: injecting the shared transport is a connection-reuse
    // performance optimization. A client built with default options round-trips
    // identically against Azure (only its pooling differs), so no behavioral test
    // can distinguish carrying the shared transport from not, and a mutant that
    // drops it makes the SDK build a default client that hangs the round-trip.
    #[cfg_attr(test, mutants::skip)]
    fn shared_client_options(&self) -> ClientOptions {
        ClientOptions {
            transport: Some(Transport::new(Arc::clone(&self.http_client))),
            ..Default::default()
        }
    }

    /// Builds a client for the blob named `key`, constructing the URL one path
    /// segment at a time so `/` separators in the key stay literal.
    // Mutation-skipped: the surviving mutant only drops the shared-transport
    // client options, an unobservable connection-reuse optimization (see
    // `shared_client_options`). The URL plumbing is exercised by the Azurite
    // round-trip and prefix-escape tests, which mutation testing cannot run.
    #[cfg_attr(test, mutants::skip)]
    fn blob_client(&self, key: &str) -> Result<BlobClient, StorageError> {
        let mut url = self.container_endpoint.clone();
        url.path_segments_mut()
            .map_err(|()| io_error("Azure endpoint cannot be a base URL"))?
            .extend(key.split('/'));
        let options = BlobClientOptions {
            client_options: self.shared_client_options(),
            ..Default::default()
        };
        BlobClient::new(url, self.credential.clone(), Some(options))
            .map_err(|error| azure_io(&error))
    }

    /// Builds a client for the configured container.
    // Mutation-skipped: the surviving mutant only drops the shared-transport
    // client options, an unobservable connection-reuse optimization (see
    // `shared_client_options`). The container round-trip is covered by the
    // Azurite tests, which mutation testing cannot run.
    #[cfg_attr(test, mutants::skip)]
    fn container_client(&self) -> Result<BlobContainerClient, StorageError> {
        let options = BlobContainerClientOptions {
            client_options: self.shared_client_options(),
            ..Default::default()
        };
        BlobContainerClient::new(
            self.container_endpoint.clone(),
            self.credential.clone(),
            Some(options),
        )
        .map_err(|error| azure_io(&error))
    }

    /// Creates the container, treating an already-existing container as success.
    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn ensure_container(&self) -> Result<(), StorageError> {
        let client = self.container_client()?;
        match client.create(None).await {
            Ok(_) => Ok(()),
            Err(error) if matches!(classify(&error), Fault::AlreadyExists) => Ok(()),
            Err(error) => Err(azure_io(&error)),
        }
    }

    /// Uploads `bytes` at `client`, creating the container and retrying once if it
    /// does not exist yet. `if_not_exists` selects write-once versus replacing
    /// semantics (see [`upload`]).
    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn upload_with_retry(
        &self,
        client: &BlobClient,
        bytes: &[u8],
        key: &str,
        if_not_exists: bool,
    ) -> Result<(), StorageError> {
        match upload(client, bytes, if_not_exists).await {
            Ok(()) => Ok(()),
            Err(error) if matches!(classify(&error), Fault::ContainerMissing) => {
                // The container does not exist yet; create it and retry once.
                self.ensure_container().await?;
                upload(client, bytes, if_not_exists)
                    .await
                    .map_err(|error| map_error(&error, key))
            }
            Err(error) => Err(map_error(&error, key)),
        }
    }
}

impl Storage for AzureBlobStorage {
    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn put(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_key(key)?;
        let client = self.blob_client(key)?;
        let compressed = codec::compress(bytes);
        self.upload_with_retry(&client, &compressed, key, true)
            .await
    }

    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn put_overwrite(&self, key: &str, bytes: &[u8]) -> Result<(), StorageError> {
        validate_key(key)?;
        let client = self.blob_client(key)?;
        let compressed = codec::compress(bytes);
        self.upload_with_retry(&client, &compressed, key, false)
            .await
    }

    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        validate_key(key)?;
        let client = self.blob_client(key)?;
        match client.download(None).await {
            Ok(response) => {
                let bytes = response
                    .body
                    .collect()
                    .await
                    .map_err(|error| azure_io(&error))?;
                codec::decompress(&bytes).map_err(StorageError::Io)
            }
            Err(error) if matches!(classify(&error), Fault::NotFound | Fault::ContainerMissing) => {
                Err(StorageError::NotFound {
                    key: key.to_owned(),
                })
            }
            Err(error) => Err(azure_io(&error)),
        }
    }

    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        let client = self.container_client()?;
        let mut pager = client
            .list_blobs(Some(BlobContainerClientListBlobsOptions {
                prefix: Some(prefix.to_owned()),
                ..Default::default()
            }))
            .map_err(|error| azure_io(&error))?;

        let mut keys = Vec::new();
        loop {
            match pager.try_next().await {
                Ok(Some(item)) => {
                    if let Some(name) = item.name {
                        keys.push(name);
                    }
                }
                Ok(None) => break,
                // A missing container holds no objects, mirroring a missing local
                // storage root listing as empty rather than erroring.
                Err(error) if matches!(classify(&error), Fault::ContainerMissing) => {
                    return Ok(Vec::new());
                }
                Err(error) => return Err(azure_io(&error)),
            }
        }
        keys.sort();
        Ok(keys)
    }

    #[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        validate_key(key)?;
        let client = self.blob_client(key)?;
        match client.delete(None).await {
            Ok(_) => Ok(()),
            Err(error) if matches!(classify(&error), Fault::NotFound | Fault::ContainerMissing) => {
                Err(StorageError::NotFound {
                    key: key.to_owned(),
                })
            }
            Err(error) => Err(azure_io(&error)),
        }
    }
}

/// Resolves the Entra ID token credential used against the HTTPS Blob endpoint.
///
/// In a GitHub Actions job configured for Azure federation it self-mints fresh OIDC
/// assertions (see [`github_oidc`]), which keeps a long collection run authenticated
/// past the first hourly access-token refresh — a single `azure/login` session
/// cannot, because the assertion it caches expires within minutes. Everywhere else
/// (local development, and the `test-azure` CI job that signs in with `azure/login`)
/// it falls back to [`DeveloperToolsCredential`], which discovers the existing
/// Azure CLI session.
fn entra_credential(
    http_client: &Arc<dyn HttpClient>,
) -> Result<Arc<dyn TokenCredential>, StorageError> {
    if let Some(result) = github_oidc::from_env(http_client) {
        return result;
    }
    let credential: Arc<dyn TokenCredential> =
        DeveloperToolsCredential::new(None).map_err(|error| {
            config_error(format!("could not initialize Entra ID credential: {error}"))
        })?;
    Ok(credential)
}

/// How long before a cached token's stated expiry it is treated as stale and
/// re-acquired, so a token is never handed out only to expire while the request
/// that just read it is still in flight.
const TOKEN_REFRESH_MARGIN: Duration = Duration::minutes(5);

/// A [`TokenCredential`] decorator that serializes token acquisition and caches
/// each scope's most recent token, so a burst of concurrent reads shares one
/// token instead of each driving a separate acquisition of the wrapped
/// credential.
///
/// The Entra credential ([`DeveloperToolsCredential`]) shells out to the Azure
/// CLI on every uncached `get_token`. Without this decorator, `analyze`'s
/// concurrent object loads fan out into that many simultaneous
/// `az account get-access-token` invocations; on Windows they collide on the
/// exclusive MSAL token-cache lockfile and the whole read fails with a permission
/// error. Holding the cache lock across the inner acquisition means only one
/// acquisition is ever in flight, so later callers reuse the freshly cached token
/// rather than racing the CLI.
struct CachingCredential {
    /// The wrapped credential that performs the real token acquisition.
    inner: Arc<dyn TokenCredential>,
    /// The most recent token per requested scope set. The lock is deliberately
    /// held across the inner acquisition, so concurrent callers serialize on it
    /// and share a single fetch rather than stampeding the wrapped credential.
    cache: futures::lock::Mutex<HashMap<Vec<String>, AccessToken>>,
    /// The wall-clock source used to decide whether a cached token is still
    /// fresh. Injected so the freshness logic is deterministic under test.
    now: fn() -> OffsetDateTime,
}

impl fmt::Debug for CachingCredential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachingCredential")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

impl CachingCredential {
    /// Wraps `inner`, reading the real wall clock for token-freshness decisions.
    fn new(inner: Arc<dyn TokenCredential>) -> Self {
        Self::with_clock(inner, OffsetDateTime::now_utc)
    }

    /// Wraps `inner` with an explicit clock, so tests pin token freshness
    /// deterministically instead of reading the wall clock.
    fn with_clock(inner: Arc<dyn TokenCredential>, now: fn() -> OffsetDateTime) -> Self {
        Self {
            inner,
            cache: futures::lock::Mutex::new(HashMap::new()),
            now,
        }
    }
}

#[async_trait::async_trait]
impl TokenCredential for CachingCredential {
    async fn get_token(
        &self,
        scopes: &[&str],
        options: Option<TokenRequestOptions<'_>>,
    ) -> azure_core::Result<AccessToken> {
        let cache_key: Vec<String> = scopes.iter().map(|scope| (*scope).to_owned()).collect();

        // Holding the lock across the inner acquisition is the whole point: it
        // collapses a concurrent burst into one acquisition whose result the
        // other callers then read from the cache.
        let mut cache = self.cache.lock().await;
        if let Some(token) = cache.get(&cache_key)
            && token_is_fresh(token, (self.now)())
        {
            return Ok(token.clone());
        }

        let token = self.inner.get_token(scopes, options).await?;
        cache.insert(cache_key, token.clone());
        Ok(token)
    }
}

/// Whether `token` is still far enough from its expiry to reuse at `now`,
/// keeping a [`TOKEN_REFRESH_MARGIN`] safety margin so it cannot expire while a
/// request that just read it is still in flight.
///
/// A `now` so close to the maximum representable date that adding the margin
/// overflows is treated as not fresh (re-acquire), which is the safe default.
fn token_is_fresh(token: &AccessToken, now: OffsetDateTime) -> bool {
    now.checked_add(TOKEN_REFRESH_MARGIN)
        .is_some_and(|deadline| token.expires_on > deadline)
}

/// Uploads `bytes` to `client`, creating the container and retrying once if it
/// does not exist yet. When `if_not_exists` is set the upload is write-once
/// (failing if the blob already exists); otherwise it replaces any existing blob.
#[cfg_attr(test, mutants::skip)] // Delegates to the Azure SDK; verified by the Azurite round-trip tests, which mutation testing cannot run.
async fn upload(client: &BlobClient, bytes: &[u8], if_not_exists: bool) -> azure_core::Result<()> {
    let mut options = BlobClientUploadOptions {
        // The body is always gzip, so declare it: a non-SDK reader can then
        // inflate the blob with standard tooling.
        blob_content_encoding: Some(GZIP_CONTENT_ENCODING.to_owned()),
        ..Default::default()
    };
    if if_not_exists {
        options = options.if_not_exists();
    }
    client
        .upload(RequestContent::from(bytes.to_vec()), Some(options))
        .await?;
    Ok(())
}

/// The SAS expiry, a fixed lifetime from now, formatted as `YYYY-MM-DDThh:mm:ssZ`.
fn account_sas_expiry() -> String {
    let expiry = Timestamp::now()
        .checked_add(24.hours())
        .expect("the current time plus a fixed lifetime is representable");
    expiry
        .to_zoned(TimeZone::UTC)
        .strftime("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

/// The SAS signed protocol for `endpoint`: `https` for a secure endpoint, or
/// `https,http` for a plain-HTTP endpoint such as the Azurite emulator.
fn sas_protocol(endpoint: &Url) -> &'static str {
    if endpoint.scheme() == "https" {
        "https"
    } else {
        "https,http"
    }
}

/// The kind of fault an Azure error represents, in terms the storage model cares
/// about.
#[derive(Clone, Copy)]
enum Fault {
    /// The requested blob does not exist.
    NotFound,
    /// The container does not exist.
    ContainerMissing,
    /// The blob already exists (a write-once conflict).
    AlreadyExists,
    /// Any other failure.
    Other,
}

/// Classifies an Azure error by HTTP status and storage error code.
fn classify(error: &azure_core::Error) -> Fault {
    let code = match error.kind() {
        ErrorKind::HttpResponse { error_code, .. } => error_code.as_deref(),
        _ => None,
    };
    if code == Some(StorageErrorCode::ContainerNotFound.as_ref()) {
        return Fault::ContainerMissing;
    }
    match error.http_status() {
        Some(StatusCode::NotFound) => Fault::NotFound,
        Some(StatusCode::Conflict | StatusCode::PreconditionFailed) => Fault::AlreadyExists,
        _ => Fault::Other,
    }
}

/// Maps an Azure error to a [`StorageError`] for the object identified by `key`.
fn map_error(error: &azure_core::Error, key: &str) -> StorageError {
    match classify(error) {
        Fault::NotFound | Fault::ContainerMissing => StorageError::NotFound {
            key: key.to_owned(),
        },
        Fault::AlreadyExists => StorageError::AlreadyExists {
            key: key.to_owned(),
        },
        Fault::Other => azure_io(error),
    }
}

/// Wraps an Azure error as a generic storage I/O error.
///
/// Formats with `Debug` rather than `Display` on purpose: the SDK's retry policy
/// replaces the `Display` text with an opaque "non-transport error occurred which
/// will not be retried", masking the real cause (for example a credential
/// acquisition failure). The `Debug` representation preserves the full error
/// chain, so the underlying fault is visible in diagnostics.
fn azure_io(error: &azure_core::Error) -> StorageError {
    StorageError::Io(io::Error::other(format!("Azure Blob error: {error:?}")))
}

/// Builds a generic storage I/O error from a static message.
fn io_error(message: &'static str) -> StorageError {
    StorageError::Io(io::Error::other(message))
}

/// Builds a storage configuration error.
fn config_error(message: impl Into<String>) -> StorageError {
    StorageError::Config {
        message: message.into(),
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    use futures::executor::block_on;

    /// The well-known Azurite development account key (public, fixed, not secret).
    const AZURITE_KEY: &str =
        "Eby8vdM02xNOcqFlqUwJPLlmEtlCDXJ1OUzFT50uSRZ6IFsuFq2UVErCz4I6tq/K1SZFPTOtr/KBHBeksoGMGw==";

    /// Builds an Azure HTTP-response error with the given status and storage
    /// error code, mirroring what the SDK surfaces for a failed request.
    fn http_error(status: StatusCode, error_code: Option<&str>) -> azure_core::Error {
        ErrorKind::HttpResponse {
            status,
            error_code: error_code.map(str::to_owned),
            raw_response: None,
        }
        .into_error()
    }

    #[test]
    fn classify_maps_container_not_found_code_to_container_missing() {
        let error = http_error(
            StatusCode::NotFound,
            Some(StorageErrorCode::ContainerNotFound.as_ref()),
        );
        assert!(matches!(classify(&error), Fault::ContainerMissing));
    }

    #[test]
    fn classify_maps_not_found_status_without_container_code_to_not_found() {
        let error = http_error(StatusCode::NotFound, None);
        assert!(matches!(classify(&error), Fault::NotFound));
    }

    #[test]
    fn classify_maps_conflict_status_to_already_exists() {
        let error = http_error(StatusCode::Conflict, None);
        assert!(matches!(classify(&error), Fault::AlreadyExists));
    }

    #[test]
    fn classify_maps_precondition_failed_status_to_already_exists() {
        let error = http_error(StatusCode::PreconditionFailed, None);
        assert!(matches!(classify(&error), Fault::AlreadyExists));
    }

    #[test]
    fn classify_maps_other_status_to_other() {
        let error = http_error(StatusCode::InternalServerError, None);
        assert!(matches!(classify(&error), Fault::Other));
    }

    #[test]
    fn classify_maps_non_http_error_to_other() {
        let error = ErrorKind::Io.into_error();
        assert!(matches!(classify(&error), Fault::Other));
    }

    #[test]
    fn map_error_distinguishes_not_found_already_exists_and_io() {
        let key = "object";
        assert!(matches!(
            map_error(&http_error(StatusCode::NotFound, None), key),
            StorageError::NotFound { .. }
        ));
        assert!(matches!(
            map_error(&http_error(StatusCode::Conflict, None), key),
            StorageError::AlreadyExists { .. }
        ));
        assert!(matches!(
            map_error(&http_error(StatusCode::InternalServerError, None), key),
            StorageError::Io(_)
        ));
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn account_sas_expiry_is_a_future_fixed_width_utc_timestamp() {
        let expiry = account_sas_expiry();
        let parsed: Timestamp = expiry.parse().unwrap();
        assert!(
            parsed > Timestamp::now(),
            "expiry {expiry} should be in the future"
        );
        // The SAS string-to-sign requires the fixed-width `YYYY-MM-DDThh:mm:ssZ` form.
        assert_eq!(expiry.len(), "2030-01-01T00:00:00Z".len(), "{expiry}");
        assert!(expiry.ends_with('Z'), "{expiry}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn account_key_mode_bakes_a_sas_query_and_uses_no_credential() {
        let storage = AzureBlobStorage::from_config(
            "devstoreaccount1",
            "bench-history",
            Some("http://127.0.0.1:10000/devstoreaccount1".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();

        assert!(storage.credential.is_none());
        let query = storage.container_endpoint.query().unwrap();
        assert!(query.contains("sig="), "{query}");
        assert!(query.contains("spr=https%2Chttp"), "{query}");
        assert_eq!(
            storage.container_endpoint.path(),
            "/devstoreaccount1/bench-history"
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn sas_token_mode_uses_the_token_verbatim() {
        let storage = AzureBlobStorage::from_config(
            "prod",
            "history",
            Some("https://prod.blob.core.windows.net".to_owned()),
            None,
            Some("?sv=2021-08-06&sig=abc".to_owned()),
        )
        .unwrap();

        assert!(storage.credential.is_none());
        // The leading `?` is stripped; the rest is used as the query verbatim.
        assert_eq!(
            storage.container_endpoint.query(),
            Some("sv=2021-08-06&sig=abc")
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn entra_mode_uses_a_credential_and_default_endpoint() {
        let storage = AzureBlobStorage::from_config("prod", "history", None, None, None).unwrap();

        assert!(storage.credential.is_some());
        assert_eq!(storage.container_endpoint.query(), None);
        assert_eq!(
            storage.container_endpoint.as_str(),
            "https://prod.blob.core.windows.net/history"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn blob_client_keeps_slash_separators_literal() {
        let storage = AzureBlobStorage::from_config(
            "devstoreaccount1",
            "bench-history",
            Some("http://127.0.0.1:10000/devstoreaccount1".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();

        let client = storage.blob_client("v1/proj/callgrind/run.json").unwrap();
        assert_eq!(
            client.url().path(),
            "/devstoreaccount1/bench-history/v1/proj/callgrind/run.json"
        );
        // The SAS query is carried over to the blob URL.
        assert!(client.url().query().is_some());
    }

    #[test]
    fn both_auth_modes_set_is_a_config_error() {
        let error = AzureBlobStorage::from_config(
            "prod",
            "history",
            None,
            Some(AZURITE_KEY.to_owned()),
            Some("sig=abc".to_owned()),
        )
        .unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    fn entra_over_http_is_a_config_error() {
        let error = AzureBlobStorage::from_config(
            "prod",
            "history",
            Some("http://insecure.example/account".to_owned()),
            None,
            None,
        )
        .unwrap_err();
        match error {
            StorageError::Config { message } => {
                assert!(message.contains("https"), "{message}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn invalid_endpoint_is_a_config_error() {
        let error = AzureBlobStorage::from_config(
            "prod",
            "history",
            Some("not a url".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn invalid_account_key_is_a_config_error() {
        let error = AzureBlobStorage::from_config(
            "devstoreaccount1",
            "bench-history",
            Some("http://127.0.0.1:10000/devstoreaccount1".to_owned()),
            Some("not valid base64!!!".to_owned()),
            None,
        )
        .unwrap_err();
        assert!(matches!(error, StorageError::Config { .. }), "{error:?}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn https_endpoint_signs_with_https_only_protocol() {
        let storage = AzureBlobStorage::from_config(
            "prod",
            "history",
            Some("https://prod.blob.core.windows.net".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();
        let query = storage.container_endpoint.query().unwrap();
        assert!(
            query.contains("spr=https&") || query.ends_with("spr=https"),
            "{query}"
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn debug_redacts_the_sas_query() {
        let storage = AzureBlobStorage::from_config(
            "devstoreaccount1",
            "bench-history",
            Some("http://127.0.0.1:10000/devstoreaccount1".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();
        let rendered = format!("{storage:?}");
        assert!(
            !rendered.contains("sig="),
            "must not leak signature: {rendered}"
        );
        assert!(rendered.contains("bench-history"), "{rendered}");
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads the wall clock to compute the SAS expiry")]
    fn trailing_slash_endpoint_does_not_double_up_segments() {
        let storage = AzureBlobStorage::from_config(
            "devstoreaccount1",
            "bench-history",
            Some("http://127.0.0.1:10000/devstoreaccount1/".to_owned()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();
        assert_eq!(
            storage.container_endpoint.path(),
            "/devstoreaccount1/bench-history"
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "builds a real HTTP client (reqwest) that Miri cannot run"
    )]
    fn put_and_get_reject_keys_that_escape_the_prefix() {
        // The Azure IO methods delegate to the SDK (and are mutation-skipped), but
        // each first runs the pure `validate_key` guard before any network call.
        // `block_on` drives that guard to completion without an emulator or a Tokio
        // runtime: the future resolves to the rejection before it awaits anything.
        let storage = AzureBlobStorage::from_config(
            "prod",
            "history",
            Some("https://prod.blob.core.windows.net".to_owned()),
            None,
            Some("?sv=2021-08-06&sig=abc".to_owned()),
        )
        .unwrap();

        let put = block_on(storage.put("../bad", b"x")).unwrap_err();
        assert!(matches!(put, StorageError::InvalidKey { .. }), "{put:?}");
        let get = block_on(storage.get("../bad")).unwrap_err();
        assert!(matches!(get, StorageError::InvalidKey { .. }), "{get:?}");
    }

    // =======================================================================
    // Caching-credential tests.
    //
    // These cover the token caching/serialization decorator with a fake inner
    // credential, so they exercise the dedup, freshness, and per-scope behaviour
    // without a real Entra credential or any wall-clock read (the clock is
    // injected). They stay Miri-safe: no IO, no real time.
    //
    // `AtomicU64` and `Ordering` are imported by the Azurite section below (one
    // `tests` module, so module-scoped). `Future` is in the 2024 prelude.
    // =======================================================================

    use std::pin::Pin;
    use std::task::{Context, Poll};

    use futures::future::join_all;

    /// The Unix-second anchor shared by [`fixed_now`] and [`at_offset`].
    const BASE_UNIX: i64 = 1_000_000_000;

    /// A fixed "now" used by the caching-credential tests so token freshness is
    /// deterministic and Miri-safe (no wall-clock read).
    fn fixed_now() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(BASE_UNIX).unwrap()
    }

    /// Builds an [`OffsetDateTime`] `offset` seconds from [`fixed_now`].
    fn at_offset(offset: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(BASE_UNIX.saturating_add(offset)).unwrap()
    }

    /// Yields control back to the executor exactly once, so a concurrently-polled
    /// burst can interleave inside the fake credential's acquisition (and thus be
    /// observed as concurrent if it were ever allowed to run unserialized).
    struct YieldOnce(bool);

    impl Future for YieldOnce {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// A fake [`TokenCredential`] that records how many times — and how
    /// concurrently — it is asked to acquire a token, optionally yielding once
    /// mid-acquisition so a burst can be observed as concurrent if it is not
    /// serialized by the decorator.
    #[derive(Debug)]
    struct CountingCredential {
        calls: AtomicU64,
        in_flight: AtomicU64,
        max_in_flight: AtomicU64,
        yield_during_acquire: bool,
        expires_at_unix: i64,
    }

    impl CountingCredential {
        fn new(expires_at_unix: i64, yield_during_acquire: bool) -> Self {
            Self {
                calls: AtomicU64::new(0),
                in_flight: AtomicU64::new(0),
                max_in_flight: AtomicU64::new(0),
                yield_during_acquire,
                expires_at_unix,
            }
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::Relaxed)
        }

        fn max_in_flight(&self) -> u64 {
            self.max_in_flight.load(Ordering::Relaxed)
        }
    }

    #[async_trait::async_trait]
    impl TokenCredential for CountingCredential {
        async fn get_token(
            &self,
            _scopes: &[&str],
            _options: Option<TokenRequestOptions<'_>>,
        ) -> azure_core::Result<AccessToken> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let now = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
            self.max_in_flight.fetch_max(now, Ordering::Relaxed);
            if self.yield_during_acquire {
                YieldOnce(false).await;
            }
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            Ok(AccessToken::new(
                "fake-token",
                OffsetDateTime::from_unix_timestamp(self.expires_at_unix).unwrap(),
            ))
        }
    }

    #[test]
    fn token_is_fresh_accepts_a_token_beyond_the_refresh_margin() {
        let margin = TOKEN_REFRESH_MARGIN.whole_seconds();
        let token = AccessToken::new("t", at_offset(margin + 1));
        assert!(token_is_fresh(&token, fixed_now()));
    }

    #[test]
    fn token_is_fresh_rejects_a_token_within_the_refresh_margin() {
        let margin = TOKEN_REFRESH_MARGIN.whole_seconds();
        let token = AccessToken::new("t", at_offset(margin - 1));
        assert!(!token_is_fresh(&token, fixed_now()));
    }

    #[test]
    fn token_is_fresh_rejects_a_token_exactly_at_the_refresh_margin() {
        let margin = TOKEN_REFRESH_MARGIN.whole_seconds();
        let token = AccessToken::new("t", at_offset(margin));
        assert!(!token_is_fresh(&token, fixed_now()));
    }

    #[test]
    fn token_is_fresh_rejects_an_expired_token() {
        let token = AccessToken::new("t", at_offset(-1));
        assert!(!token_is_fresh(&token, fixed_now()));
    }

    #[test]
    fn counting_credential_observes_concurrency_when_unserialized() {
        // Establishes the discriminating power of the burst test: hit the fake
        // directly (no decorator) and the yield lets the burst overlap, so the
        // observed max concurrency exceeds one.
        let inner = Arc::new(CountingCredential::new(
            at_offset(3600).unix_timestamp(),
            true,
        ));
        let scopes = ["scope"];
        let calls = std::iter::repeat_with(|| inner.get_token(&scopes, None)).take(8);
        for result in block_on(join_all(calls)) {
            result.unwrap();
        }
        assert_eq!(inner.calls(), 8);
        assert!(inner.max_in_flight() > 1, "{}", inner.max_in_flight());
    }

    #[test]
    fn caching_credential_collapses_a_concurrent_burst_into_one_acquisition() {
        let inner = Arc::new(CountingCredential::new(
            at_offset(3600).unix_timestamp(),
            true,
        ));
        let cred =
            CachingCredential::with_clock(Arc::<CountingCredential>::clone(&inner), fixed_now);
        let scopes = ["scope"];
        let calls = std::iter::repeat_with(|| cred.get_token(&scopes, None)).take(8);
        for result in block_on(join_all(calls)) {
            result.unwrap();
        }
        // Serialized through the cache lock: a single acquisition, never overlapping.
        assert_eq!(inner.calls(), 1);
        assert_eq!(inner.max_in_flight(), 1);
    }

    #[test]
    fn caching_credential_reuses_a_fresh_token() {
        let inner = Arc::new(CountingCredential::new(
            at_offset(3600).unix_timestamp(),
            false,
        ));
        let cred =
            CachingCredential::with_clock(Arc::<CountingCredential>::clone(&inner), fixed_now);
        let scopes = ["scope"];
        block_on(cred.get_token(&scopes, None)).unwrap();
        block_on(cred.get_token(&scopes, None)).unwrap();
        assert_eq!(inner.calls(), 1);
    }

    #[test]
    fn caching_credential_reacquires_an_expired_token() {
        // The fake hands back an already-stale token, so every call must refresh.
        let inner = Arc::new(CountingCredential::new(
            at_offset(-10).unix_timestamp(),
            false,
        ));
        let cred =
            CachingCredential::with_clock(Arc::<CountingCredential>::clone(&inner), fixed_now);
        let scopes = ["scope"];
        block_on(cred.get_token(&scopes, None)).unwrap();
        block_on(cred.get_token(&scopes, None)).unwrap();
        assert_eq!(inner.calls(), 2);
    }

    #[test]
    fn caching_credential_caches_each_scope_independently() {
        let inner = Arc::new(CountingCredential::new(
            at_offset(3600).unix_timestamp(),
            false,
        ));
        let cred =
            CachingCredential::with_clock(Arc::<CountingCredential>::clone(&inner), fixed_now);
        block_on(cred.get_token(&["scope-a"], None)).unwrap();
        block_on(cred.get_token(&["scope-b"], None)).unwrap();
        // Each new scope acquires once; re-requesting a cached scope does not.
        assert_eq!(inner.calls(), 2);
        block_on(cred.get_token(&["scope-a"], None)).unwrap();
        assert_eq!(inner.calls(), 2);
    }

    #[test]
    fn caching_credential_debug_hides_the_cache_and_clock() {
        let inner: Arc<dyn TokenCredential> = Arc::new(CountingCredential::new(
            at_offset(3600).unix_timestamp(),
            false,
        ));
        let cred = CachingCredential::with_clock(inner, fixed_now);
        let rendered = format!("{cred:?}");
        assert!(rendered.contains("CachingCredential"), "{rendered}");
    }

    // =======================================================================
    // Network tests against a live Azurite emulator.
    //
    // These exercise the real put/get/list paths and the error classification
    // that pure tests cannot reach. They require an Azurite blob endpoint (the
    // CI `test-azurite` job provides one; see the package AGENTS.md for running
    // them locally) and so are network-bound: ignored under Miri and serialized.
    // Each test uses a freshly named container, so they never share state even
    // against a shared emulator.
    // =======================================================================

    use std::net::{TcpStream, ToSocketAddrs as _};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use serial_test::serial;

    /// The Azurite blob endpoint, overridable for a non-default emulator.
    fn azurite_endpoint() -> String {
        std::env::var("AZURITE_BLOB_ENDPOINT")
            .unwrap_or_else(|_| "http://127.0.0.1:10000/devstoreaccount1".to_owned())
    }

    /// A fresh, valid container name (lowercase, 3-63 chars) unique to one test.
    fn unique_container() -> String {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = Timestamp::now().as_nanosecond();
        format!("bh-test-{nanos}-{n}")
    }

    /// Whether an Azurite blob endpoint is reachable via a short TCP connect.
    ///
    /// These tests are always compiled, but the runner usually has no emulator. A
    /// reachability probe lets the test self-skip there while still running for
    /// real wherever Azurite is provided.
    fn azurite_reachable() -> bool {
        let endpoint = azurite_endpoint();
        let Ok(url) = Url::parse(&endpoint) else {
            return false;
        };
        let host = url.host_str().unwrap_or("127.0.0.1").to_owned();
        let port = url.port().unwrap_or(10000);
        let Ok(addrs) = (host.as_str(), port).to_socket_addrs() else {
            return false;
        };
        addrs
            .into_iter()
            .any(|addr| TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok())
    }

    /// An account-key backend for a fresh container, or `None` to skip when no
    /// emulator is reachable.
    ///
    /// Setting `BENCH_HISTORY_REQUIRE_AZURITE` turns an unreachable emulator into
    /// a hard failure, so the dedicated CI job that provisions Azurite cannot
    /// silently degrade into skipping every network test.
    fn azurite_storage_or_skip() -> Option<AzureBlobStorage> {
        if !azurite_reachable() {
            assert!(
                std::env::var_os("BENCH_HISTORY_REQUIRE_AZURITE").is_none(),
                "BENCH_HISTORY_REQUIRE_AZURITE is set but no Azurite emulator is reachable at {}",
                azurite_endpoint()
            );
            eprintln!(
                "skipping Azurite network test: no emulator reachable at {}",
                azurite_endpoint()
            );
            return None;
        }
        let storage = AzureBlobStorage::from_config(
            "devstoreaccount1",
            &unique_container(),
            Some(azurite_endpoint()),
            Some(AZURITE_KEY.to_owned()),
            None,
        )
        .unwrap();
        Some(storage)
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn put_creates_the_container_then_get_and_list_round_trip() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };

        // The very first put hits a container that does not exist yet, so this
        // also covers the create-on-demand retry path.
        storage.put("v1/proj/a/1.json", b"first").await.unwrap();
        storage.put("v1/proj/a/2.json", b"second").await.unwrap();
        storage.put("v1/proj/b/3.json", b"third").await.unwrap();

        assert_eq!(storage.get("v1/proj/a/1.json").await.unwrap(), b"first");

        let listed = storage.list("v1/proj/a/").await.unwrap();
        assert_eq!(
            listed,
            vec!["v1/proj/a/1.json".to_owned(), "v1/proj/a/2.json".to_owned()]
        );

        let all = storage.list("v1/proj/").await.unwrap();
        assert_eq!(all.len(), 3, "{all:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn get_missing_blob_in_existing_container_reports_not_found() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // Create the container (and an unrelated blob) so the missing-blob case
        // is a blob-level 404, not a missing container.
        storage.put("v1/present.json", b"x").await.unwrap();

        let error = storage.get("v1/absent.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn get_in_missing_container_reports_not_found() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // No put, so the container does not exist: a get must still resolve to a
        // plain not-found rather than an I/O error.
        let error = storage.get("v1/absent.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn delete_removes_a_blob_and_leaves_siblings() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        storage.put("v1/proj/clean.json", b"c").await.unwrap();
        storage.put("v1/proj/dirty.json", b"d").await.unwrap();

        storage.delete("v1/proj/dirty.json").await.unwrap();

        // The sibling survives and the deleted blob reports not-found.
        assert_eq!(
            storage.list("v1/proj/").await.unwrap(),
            vec!["v1/proj/clean.json".to_owned()]
        );
        let error = storage.get("v1/proj/dirty.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn delete_missing_blob_reports_not_found() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // Create the container (and an unrelated blob) so the missing-blob case
        // is a blob-level 404, not a missing container.
        storage.put("v1/present.json", b"x").await.unwrap();

        let error = storage.delete("v1/absent.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn delete_in_missing_container_reports_not_found() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // No put, so the container does not exist: a delete must still resolve to
        // a plain not-found rather than an I/O error.
        let error = storage.delete("v1/absent.json").await.unwrap_err();
        assert!(matches!(error, StorageError::NotFound { .. }), "{error:?}");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn put_is_write_once() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        storage.put("v1/once.json", b"original").await.unwrap();

        let error = storage
            .put("v1/once.json", b"replacement")
            .await
            .unwrap_err();
        assert!(
            matches!(error, StorageError::AlreadyExists { .. }),
            "{error:?}"
        );
        // The original value is preserved.
        assert_eq!(storage.get("v1/once.json").await.unwrap(), b"original");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn put_overwrite_replaces_an_existing_blob() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // The first write creates the container on demand; the overwrite replaces
        // the blob's contents in place rather than failing as `put` would.
        storage.put("v1/clobber.json", b"original").await.unwrap();
        storage
            .put_overwrite("v1/clobber.json", b"replacement")
            .await
            .unwrap();

        assert_eq!(
            storage.get("v1/clobber.json").await.unwrap(),
            b"replacement"
        );
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn put_overwrite_creates_when_absent() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // No prior blob and no container: the overwrite must still create both.
        storage
            .put_overwrite("v1/fresh.json", b"only")
            .await
            .unwrap();

        assert_eq!(storage.get("v1/fresh.json").await.unwrap(), b"only");
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn list_on_missing_container_is_empty() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        // A container that was never created lists as empty, mirroring a missing
        // local storage root.
        assert!(storage.list("v1/").await.unwrap().is_empty());
    }

    #[tokio::test]
    #[cfg_attr(miri, ignore)]
    #[cfg_attr(
        mutants,
        ignore = "Azurite network test: self-skips without an emulator (as under mutation), and the IO it exercises is already mutants::skip"
    )]
    #[serial]
    async fn list_with_a_non_matching_prefix_is_empty() {
        let Some(storage) = azurite_storage_or_skip() else {
            return;
        };
        storage.put("v1/proj/a.json", b"x").await.unwrap();
        assert!(storage.list("v1/other/").await.unwrap().is_empty());
    }
}
