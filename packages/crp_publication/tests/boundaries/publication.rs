//! Real Cargo publication against an isolated registry, without production credentials.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tar::Archive;
use testing::with_watchdog_timeout;
use tiny_http::{Method, Request, Response, StatusCode};

use crate::git_fixture::Repository;
use crate::http_fixture::HttpService;
use crate::identity_fixture::LeaseService;

// These fixtures compile helper programs and packages on cold/instrumented runners.
// This deliberately generous allowance is a last-chance external-tool guard, not a timing
// assertion; normal failures are reported synchronously. Ref: docs/testing.md.
const PUBLICATION_WATCHDOG: Duration = Duration::from_mins(5);

/// Implements only the sparse-index, upload and download boundaries used by this fixture.
struct Registry {
    http: HttpService,
    state: Arc<Mutex<RegistryState>>,
}

impl Registry {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(RegistryState::default()));
        let http = HttpService::new({
            let state = Arc::clone(&state);
            move |url, request| respond(request, url, &state)
        });
        Self { http, state }
    }
}

/// Keeps uploaded bytes and index entries together so Cargo observes publication immediately.
#[derive(Default)]
struct RegistryState {
    packages: BTreeMap<String, Published>,
    order: Vec<String>,
    tokens: Vec<String>,
}

/// An uploaded package and its corresponding sparse-index record.
struct Published {
    archive: Vec<u8>,
    index: Value,
}

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo and an isolated HTTP registry")]
fn cargo_orders_workspace_publication_and_preserves_locked_binary_dependencies() {
    // Cold native compilation is normally brief; this only prevents a stuck external tool.
    with_watchdog_timeout(PUBLICATION_WATCHDOG, || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        let original_lockfile = fs::read(fixture.path().join("Cargo.lock")).unwrap();

        // Deliberately request the dependent first: Cargo owns publication ordering.
        cargo(
            &fixture,
            &[
                "publish",
                "--registry",
                "crates-io",
                "--locked",
                "-p",
                "publication-cli",
                "-p",
                "publication-core",
            ],
        );

        let (order, archive) = {
            let state = registry.state.lock().unwrap();
            // The preflight lease must not authorize either upload.
            assert_eq!(state.tokens, ["lease-2", "lease-3"]);
            (
                state.order.clone(),
                state
                    .packages
                    .get("publication-cli")
                    .map(|package| package.archive.clone()),
            )
        };
        assert_eq!(order, ["publication-core", "publication-cli"]);
        let archive = archive.unwrap();
        let files = archive_files(&archive);
        let lockfile = files
            .get("publication-cli-1.0.0/Cargo.lock")
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let packages = lockfile
            .get("package")
            .unwrap()
            .as_array_of_tables()
            .unwrap();
        let dependency = packages
            .iter()
            .find(|package| package["name"].as_str() == Some("publication-core"))
            .unwrap();
        assert_eq!(dependency["version"].as_str(), Some("1.0.0"));
        assert!(
            dependency["source"]
                .as_str()
                .unwrap()
                .contains("crates.io-index")
        );
        assert_eq!(
            fs::read(fixture.path().join("Cargo.lock")).unwrap(),
            original_lockfile
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo and an isolated HTTP registry")]
fn cargo_publishes_a_dependent_after_its_dependency_is_already_available() {
    // Native compiler startup varies across runners; no assertion waits for this deadline.
    with_watchdog_timeout(PUBLICATION_WATCHDOG, || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        for package in ["publication-core", "publication-cli"] {
            cargo(
                &fixture,
                &[
                    "publish",
                    "--registry",
                    "crates-io",
                    "--locked",
                    "-p",
                    package,
                ],
            );
        }
        let order = registry.state.lock().unwrap().order.clone();
        assert_eq!(order, ["publication-core", "publication-cli"]);
    });
}

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo and an isolated HTTP registry")]
fn packaged_binary_can_prune_an_inactive_workspace_dependency_feature() {
    with_watchdog_timeout(PUBLICATION_WATCHDOG, || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        fixture.write(
            "Cargo.toml",
            b"[workspace]\nmembers = ['core', 'cli', 'optional']\nresolver = '3'\n",
        );
        fixture.write(
            "optional/Cargo.toml",
            b"[package]\nname='publication-optional'\nversion='1.0.0'\nedition='2024'\n\
              license='MIT'\ndescription='Optional fixture dependency'\n\
              repository='https://example.invalid/fixture'\n",
        );
        fixture.write("optional/src/lib.rs", b"pub fn optional() {}\n");
        let core = fs::read_to_string(fixture.path().join("core/Cargo.toml")).unwrap();
        fixture.write(
            "core/Cargo.toml",
            format!(
                "{core}\n[dependencies]\npublication-optional = {{ path='../optional', \
                version='1.0.0', optional=true }}\n\
                [features]\nextra = ['dep:publication-optional']\n"
            )
            .as_bytes(),
        );
        cargo(&fixture, &["generate-lockfile", "--offline"]);
        fixture.command(&["add", "."]);
        fixture.command(&["commit", "--quiet", "-m", "optional workspace dependency"]);
        cargo(
            &fixture,
            &[
                "publish",
                "--registry",
                "crates-io",
                "--locked",
                "--workspace",
            ],
        );
        let archive = registry
            .state
            .lock()
            .unwrap()
            .packages
            .get("publication-cli")
            .map(|package| package.archive.clone());
        let archive = archive.unwrap();
        let files = archive_files(&archive);
        let lockfile = files
            .get("publication-cli-1.0.0/Cargo.lock")
            .unwrap()
            .parse::<toml_edit::DocumentMut>()
            .unwrap();
        let packages = lockfile
            .get("package")
            .unwrap()
            .as_array_of_tables()
            .unwrap();
        assert!(
            packages
                .iter()
                .all(|package| package["name"].as_str() != Some("publication-optional"))
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo, build scripts and a credential provider")]
fn cargo_requests_uncached_publish_credentials_after_package_verification() {
    // The events establish ordering directly, without delays or token-expiry timers.
    with_watchdog_timeout(PUBLICATION_WATCHDOG, || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        cargo(
            &fixture,
            &[
                "publish",
                "--registry",
                "crates-io",
                "--locked",
                "--workspace",
            ],
        );
        let events = fs::read_to_string(fixture.path().join("target/events.jsonl")).unwrap();
        let events: Vec<Value> = events
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let last_build = events
            .iter()
            .rposition(|event| event.get("operation").unwrap() == "build")
            .unwrap();
        let first_request = events
            .iter()
            .find(|event| event.get("kind") == Some(&json!("get")))
            .unwrap();
        assert_eq!(first_request.get("operation").unwrap(), "read");
        assert_eq!(
            first_request.pointer("/registry/index-url").unwrap(),
            "https://github.com/rust-lang/crates.io-index"
        );
        for field in ["name", "vers", "cksum"] {
            assert!(first_request.get(field).is_none());
        }
        let publication_requests: Vec<_> = events
            .iter()
            .enumerate()
            .filter(|(_, event)| event.get("operation").unwrap() == "publish")
            .collect();
        assert_eq!(
            publication_requests
                .iter()
                .map(|(_, event)| event.get("name").unwrap().as_str().unwrap())
                .collect::<Vec<_>>(),
            ["publication-core", "publication-cli"]
        );
        assert!(
            publication_requests
                .iter()
                .all(|(position, _)| *position > last_build),
            "{events:?}"
        );
        // The actual uploaded bytes, not an assumed archive path, bind each Cargo request.
        let state = registry.state.lock().unwrap();
        for (_, request) in publication_requests {
            let name = request.get("name").unwrap().as_str().unwrap();
            assert_eq!(request.get("v").unwrap(), 1);
            assert_eq!(request.get("kind").unwrap(), "get");
            assert_eq!(request.get("vers").unwrap(), "1.0.0");
            let archive = &state.packages.get(name).unwrap().archive;
            let mut checksum = String::new();
            for byte in Sha256::digest(archive) {
                write!(checksum, "{byte:02x}").unwrap();
            }
            assert_eq!(request.get("cksum").unwrap(), &checksum);
        }
    });
}

fn publication_workspace(registry: &Registry) -> Repository {
    let fixture = Repository::new();
    fixture.write(
        "Cargo.toml",
        b"[workspace]\nmembers = ['core', 'cli']\nresolver = '3'\n",
    );
    fixture.write(".gitignore", b"/target\n/cargo-home\n");
    fs::create_dir_all(fixture.path().join("target")).unwrap();
    // The loopback protocol is deterministic: Cargo retry/backoff would hide the first
    // unexpected interaction and repeat state changes rather than exercise useful recovery.
    fixture.write(
        ".cargo/config.toml",
        format!(
            "[registries.fixture]\nindex = 'sparse+{}/index/'\n\
             [net]\nretry = 0\n",
            registry.http.url()
        )
        .as_bytes(),
    );
    let metadata = "version = '1.0.0'\nedition = '2024'\nlicense = 'MIT'\n\
                    description = 'Local publication fixture'\n\
                    repository = 'https://example.invalid/fixture'\n";
    fixture.write(
        "core/Cargo.toml",
        format!("[package]\nname = 'publication-core'\n{metadata}").as_bytes(),
    );
    fixture.write("core/src/lib.rs", b"pub fn value() -> u8 { 7 }\n");
    // Model the application's synchronized package group with an exact dependency,
    // while this scenario checks Cargo publication order and packaged resolution.
    fixture.write(
        "cli/Cargo.toml",
        format!(
            "[package]\nname = 'publication-cli'\n{metadata}\n\
             [dependencies]\npublication-core = {{ version = '=1.0.0', \
             path = '../core' }}\n"
        )
        .as_bytes(),
    );
    fixture.write(
        "cli/src/main.rs",
        b"fn main() { assert_eq!(publication_core::value(), 7); }\n",
    );
    for package in ["core", "cli"] {
        fixture.write(
            &format!("{package}/build.rs"),
            include_bytes!("publication/build_events.rs"),
        );
    }
    cargo(&fixture, &["generate-lockfile", "--offline"]);
    fixture.command(&["add", "."]);
    fixture.command(&["commit", "--quiet", "-m", "publication source"]);
    fixture
}

fn cargo(fixture: &Repository, args: &[&str]) {
    let config = fs::read_to_string(fixture.path().join(".cargo/config.toml")).unwrap();
    let config: toml_edit::DocumentMut = config.parse().unwrap();
    let index = config
        .get("registries")
        .unwrap()
        .get("fixture")
        .unwrap()
        .get("index")
        .unwrap()
        .as_str()
        .unwrap();
    let mut command = Command::new("cargo");
    command
        .args(args)
        .current_dir(fixture.path())
        .env("CARGO_HOME", fixture.path().join("cargo-home"))
        .env("CARGO_TARGET_DIR", fixture.path().join("target"))
        .env(
            "CRP_PUBLICATION_EVENTS",
            fixture.path().join("target/events.jsonl"),
        )
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_HTTP_PROXY", "")
        // The local protocol fixture serves HTTP/1.1, not cleartext HTTP/2 upgrades.
        .env("CARGO_HTTP_MULTIPLEXING", "false")
        // Cargo's own test routing redirects transport, not the crates.io credential identity.
        // Ordinary source replacement does not redirect `publish --registry crates-io`.
        // Ref: Cargo's sources/config.rs, SourceConfigMap::empty.
        .env("__CARGO_TEST_CRATES_IO_URL_DO_NOT_USE_THIS", index)
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_FIXTURE_TOKEN")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS");
    let service = LeaseService::new(false);
    let session = (args.first() == Some(&"publish")).then(|| {
        let source = fixture.command(&["rev-parse", "HEAD"]).trim().to_owned();
        let packages: Vec<_> = ["cli", "core", "optional"].into_iter().filter_map(|name| {
            let manifest = format!("{name}/Cargo.toml");
            fixture.path().join(&manifest).exists().then(|| json!({
                "name":format!("publication-{name}"),"version":"1.0.0",
                "manifest":manifest,"binary":null
            }))
        }).collect();
        let publication = PublicationManifest::new(serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":source,
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/library","release-branch":"main","targets":[]},
            "packages":packages
        })).unwrap()).unwrap();
        let upload_count = if args.contains(&"--workspace") {
            publication.publication.packages.len()
        } else {
            args.iter().filter(|argument| **argument == "-p").count()
        };
        let session = CredentialSession::new(
            serde_json::from_value(json!({
                "request_url":format!("{}/identity", service.url()), "request_token":"fixture"
            })).unwrap(),
            publication,
            fixture.path().join("Cargo.toml"),
            TrustedPublisher::with_endpoint(
                &format!("{}/tokens", service.url()),
                PublicationOutput::new("1.0.0", false, Arc::new(crp_diag::Discard))
            ).unwrap()
        ).unwrap();
        session.configure(&mut command, crp_publication_test_helper::executable()).unwrap();
        (session, upload_count)
    });
    let output = command.output().unwrap();
    let leases = service.observations();
    let upload_count = session.map(|(session, count)| {
        session.finish().unwrap();
        count
    });
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    if let Some(upload_count) = upload_count {
        let (assertions, mut tokens, mut revoked) = service.observations();
        // Cargo's preflight and each uploaded package acquire separate uncached leases.
        let lease_count = upload_count.checked_add(1).unwrap();
        assert_eq!(assertions, lease_count);
        assert_eq!(tokens.len(), lease_count);
        assert_eq!(leases.2, Vec::<String>::new());
        assert!(!tokens.is_empty());
        tokens.sort();
        revoked.sort();
        assert_eq!(tokens, revoked);
    }
}

fn archive_files(bytes: &[u8]) -> BTreeMap<String, String> {
    let mut archive = Archive::new(GzDecoder::new(bytes));
    archive
        .entries()
        .unwrap()
        .map(|entry| {
            let mut entry = entry.unwrap();
            let path = entry.path().unwrap().to_str().unwrap().to_owned();
            let mut contents = String::new();
            entry.read_to_string(&mut contents).unwrap();
            (path, contents)
        })
        .collect()
}

fn respond(mut request: Request, url: &str, state: &Mutex<RegistryState>) {
    let path = request.url().to_owned();
    if path.starts_with("/index/") {
        assert!(
            !request
                .headers()
                .iter()
                .any(|header| header.field.equiv("Authorization"))
        );
    }
    eprintln!("Local registry: {} {path}", request.method());
    let response = if path == "/index/config.json" {
        json!({"dl": format!("{url}/api/v1/crates"), "api": url})
            .to_string()
            .into_bytes()
    } else if request.method() == &Method::Put && path == "/api/v1/crates/new" {
        let token = request
            .headers()
            .iter()
            .find(|header| header.field.equiv("Authorization"))
            .unwrap()
            .value
            .as_str()
            .to_owned();
        let mut body = Vec::new();
        request.as_reader().read_to_end(&mut body).unwrap();
        let mut body = Cursor::new(body);
        let metadata: Value = serde_json::from_slice(&read_field(&mut body)).unwrap();
        let archive = read_field(&mut body);
        let name = metadata.get("name").unwrap().as_str().unwrap().to_owned();
        let dependencies: Vec<_> = metadata
            .get("deps")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|dependency| {
                json!({
                    "name": dependency["name"], "req": dependency["version_req"],
                    "features": dependency["features"], "optional": dependency["optional"],
                    "default_features": dependency["default_features"],
                    "target": dependency["target"], "kind": dependency["kind"],
                    "registry": dependency["registry"]
                })
            })
            .collect();
        let mut checksum = String::new();
        for byte in Sha256::digest(&archive) {
            write!(checksum, "{byte:02x}").unwrap();
        }
        let index = json!({
            "name": name, "vers": metadata.get("vers").unwrap(), "deps": dependencies,
            "cksum": checksum,
            "features": {}, "features2": metadata.get("features").unwrap(),
            "v": 2, "yanked": false
        });
        let previous = {
            let mut state = state.lock().unwrap();
            state.tokens.push(token);
            state.order.push(name.clone());
            state.packages.insert(name, Published { archive, index })
        };
        assert!(previous.is_none());
        b"{\"warnings\":{\"invalid_categories\":[],\"invalid_badges\":[],\"other\":[]}}".to_vec()
    } else if path.starts_with("/index/") {
        let name = path.rsplit('/').next().unwrap();
        let index = state
            .lock()
            .unwrap()
            .packages
            .get(name)
            .map(|package| package.index.clone());
        if let Some(index) = index {
            format!("{index}\n").into_bytes()
        } else {
            request.respond(Response::empty(StatusCode(404))).unwrap();
            return;
        }
    } else if path.ends_with("/download") {
        let name = path.split('/').nth(4).unwrap();
        let archive = state
            .lock()
            .unwrap()
            .packages
            .get(name)
            .map(|package| package.archive.clone());
        archive.unwrap()
    } else {
        panic!("Unexpected registry request: {path}");
    };
    request.respond(Response::from_data(response)).unwrap();
}

fn read_field(body: &mut Cursor<Vec<u8>>) -> Vec<u8> {
    // Cargo's upload framing prefixes each field with its little-endian u32 length.
    let mut length = [0; size_of::<u32>()];
    body.read_exact(&mut length).unwrap();
    let mut value = vec![0; usize::try_from(u32::from_le_bytes(length)).unwrap()];
    body.read_exact(&mut value).unwrap();
    value
}
use crp_publication::PublicationOutput;
use crp_publication::publication::credentials::CredentialSession;
use crp_publication::publication::identity::TrustedPublisher;
use crp_publication::publication::manifest::PublicationManifest;
