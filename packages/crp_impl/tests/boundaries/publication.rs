//! Real Cargo publication against an isolated registry, without production credentials.

use std::collections::BTreeMap;
use std::env::consts::EXE_SUFFIX;
use std::fmt::Write as _;
use std::fs;
use std::io::{Cursor, Read};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use flate2::read::GzDecoder;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tar::Archive;
use testing::with_watchdog_timeout;
use tiny_http::{Method, Request, Response, Server, StatusCode};

use crate::git_fixture::Repository;

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo and an isolated HTTP registry")]
fn cargo_orders_workspace_publication_and_preserves_locked_binary_dependencies() {
    // Cold native compilation is normally brief; this only prevents a stuck external tool.
    with_watchdog_timeout(Duration::from_mins(5), || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        let original_lockfile = fs::read(fixture.path().join("Cargo.lock")).unwrap();

        // Deliberately request the dependent first: Cargo owns publication ordering.
        cargo(
            &fixture,
            &[
                "publish",
                "--registry",
                "fixture",
                "--locked",
                "-p",
                "publication-cli",
                "-p",
                "publication-core",
            ],
        );

        let state = registry.state.lock().unwrap();
        assert_eq!(state.order, ["publication-core", "publication-cli"]);
        let archive = &state.packages.get("publication-cli").unwrap().archive;
        let files = archive_files(archive);
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
                .contains(&registry.url)
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
    with_watchdog_timeout(Duration::from_mins(5), || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        for package in ["publication-core", "publication-cli"] {
            cargo(
                &fixture,
                &[
                    "publish",
                    "--registry",
                    "fixture",
                    "--locked",
                    "-p",
                    package,
                ],
            );
        }
        assert_eq!(
            registry.state.lock().unwrap().order,
            ["publication-core", "publication-cli"]
        );
    });
}

#[test]
#[cfg_attr(miri, ignore = "Runs Cargo, build scripts and a credential provider")]
fn cargo_requests_uncached_publish_credentials_after_package_verification() {
    // The events establish ordering directly, without delays or token-expiry timers.
    with_watchdog_timeout(Duration::from_mins(5), || {
        let registry = Registry::new();
        let fixture = publication_workspace(&registry);
        cargo(
            &fixture,
            &[
                "publish",
                "--registry",
                "fixture",
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
    });
}

fn publication_workspace(registry: &Registry) -> Repository {
    let fixture = Repository::new();
    fixture.write(
        "Cargo.toml",
        b"[workspace]\nmembers = ['core', 'cli']\nresolver = '3'\n",
    );
    fixture.write(".gitignore", b"/target\n/cargo-home\n");
    fixture.write(
        "provider.rs",
        include_bytes!("publication/credential_provider.rs"),
    );
    fs::create_dir_all(fixture.path().join("target")).unwrap();
    let provider = fixture
        .path()
        .join("target")
        .join(format!("credential-provider{EXE_SUFFIX}"));
    let output = Command::new("rustc")
        .args([
            "--edition=2024",
            "--crate-name=fixture_provider",
            "provider.rs",
            "-o",
        ])
        .arg(&provider)
        .current_dir(fixture.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let provider = serde_json::to_string(&[provider.to_str().unwrap()]).unwrap();
    fixture.write(
        ".cargo/config.toml",
        format!(
            "[registries.fixture]\nindex = 'sparse+{}/index/'\n\
             credential-provider = {provider}\n[net]\nretry = 0\n",
            registry.url
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
    fixture.write(
        "cli/Cargo.toml",
        format!(
            "[package]\nname = 'publication-cli'\n{metadata}\n\
             [dependencies]\npublication-core = {{ version = '=1.0.0', \
             path = '../core', registry = 'fixture' }}\n"
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
    let output = Command::new("cargo")
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
        .env_remove("CARGO_REGISTRY_TOKEN")
        .env_remove("CARGO_REGISTRIES_FIXTURE_TOKEN")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("RUSTFLAGS")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
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

/// Implements only the sparse-index, upload and download boundaries used by this fixture.
struct Registry {
    url: String,
    server: Arc<Server>,
    state: Arc<Mutex<RegistryState>>,
    stopped: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Registry {
    fn new() -> Self {
        let server = Arc::new(Server::http("127.0.0.1:0").unwrap());
        let url = format!("http://{}", server.server_addr());
        let state = Arc::new(Mutex::new(RegistryState::default()));
        let stopped = Arc::new(AtomicBool::new(false));
        let thread = thread::spawn({
            let server = Arc::clone(&server);
            let state = Arc::clone(&state);
            let stopped = Arc::clone(&stopped);
            let url = url.clone();
            move || {
                while !stopped.load(Ordering::Acquire) {
                    let Ok(request) = server.recv() else {
                        break;
                    };
                    respond(request, &url, &mut state.lock().unwrap());
                }
            }
        });
        Self {
            url,
            server,
            state,
            stopped,
            thread: Some(thread),
        }
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        self.server.unblock();
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

/// Keeps uploaded bytes and index entries together so Cargo observes publication immediately.
#[derive(Default)]
struct RegistryState {
    packages: BTreeMap<String, Published>,
    order: Vec<String>,
}

/// An uploaded package and its corresponding sparse-index record.
struct Published {
    archive: Vec<u8>,
    index: Value,
}

fn respond(mut request: Request, url: &str, state: &mut RegistryState) {
    let path = request.url().to_owned();
    eprintln!("Local registry: {} {path}", request.method());
    let response = if path == "/index/config.json" {
        json!({"dl": format!("{url}/api/v1/crates"), "api": url})
            .to_string()
            .into_bytes()
    } else if request.method() == &Method::Put && path == "/api/v1/crates/new" {
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
            "features": metadata.get("features").unwrap(), "yanked": false
        });
        state.order.push(name.clone());
        assert!(
            state
                .packages
                .insert(name, Published { archive, index })
                .is_none()
        );
        b"{\"warnings\":{\"invalid_categories\":[],\"invalid_badges\":[],\"other\":[]}}".to_vec()
    } else if path.starts_with("/index/") {
        let name = path.rsplit('/').next().unwrap();
        if let Some(package) = state.packages.get(name) {
            format!("{}\n", package.index).into_bytes()
        } else {
            request.respond(Response::empty(StatusCode(404))).unwrap();
            return;
        }
    } else if path.ends_with("/download") {
        let name = path.split('/').nth(4).unwrap();
        state.packages.get(name).unwrap().archive.clone()
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
