//! In-process reconciliation decisions over acquired forge and source observations.

use std::cell::{Cell, RefCell};
use std::slice;

use serde_json::json;

use super::*;
use crate::publication::binaries::model::Asset;
use crate::publication::github::candidate::tests::candidate;
use crate::publication::github::client::Release;
use crate::publication::github::outcome::tests::record;

/// Records forge calls and injects transient creation failures without network access.
#[derive(Default)]
struct FakeForge {
    tags: RefCell<BTreeMap<String, String>>,
    calls: RefCell<Vec<String>>,
    release_sources: RefCell<Vec<String>>,
    failures: Cell<usize>,
    release_exists: bool,
    created_source: Option<String>,
}

impl Forge for FakeForge {
    fn tag(&self, tag: &str) -> Result<Option<String>, AppError> {
        self.calls.borrow_mut().push(format!("tag:{tag}"));
        Ok(self.tags.borrow().get(tag).cloned())
    }

    fn create_tag(&self, tag: &str, source: &str) -> Result<(), AppError> {
        self.calls.borrow_mut().push(format!("create:{tag}"));
        if self.failures.get() > 0 {
            self.failures
                .set(self.failures.get().checked_sub(1).unwrap());
            return Err(FakeFailure::new().into());
        }
        self.tags.borrow_mut().insert(
            tag.to_owned(),
            self.created_source.as_deref().unwrap_or(source).to_owned(),
        );
        Ok(())
    }

    fn ensure_release(
        &self,
        tag: &str,
        _version: &str,
        source: &str,
        dry_run: bool,
    ) -> Result<Option<Release>, AppError> {
        self.calls.borrow_mut().push(format!("release:{tag}"));
        self.release_sources.borrow_mut().push(source.to_owned());
        if dry_run && !self.release_exists {
            return Ok(None);
        }
        Ok(Some(Release::parse(
            json!({"id":1,"tag_name":tag,"draft":false}),
            tag,
        )?))
    }

    fn assets(&self, _release: &Release) -> Result<Vec<Asset>, AppError> {
        self.calls.borrow_mut().push("assets".to_owned());
        Ok(Vec::new())
    }
}

#[ohno::error]
struct FakeFailure;

#[test]
fn competing_tag_creation_does_not_authorize_release_or_binary_publication() {
    let publication = publication();
    let forge = FakeForge {
        created_source: Some("c".repeat(40)),
        ..FakeForge::default()
    };
    let mut work = Reconciliation {
        github: &forge,
        publication: &publication,
        load_candidate: || Ok(candidate("1.0.0", true)),
        retry_pause: |_| {},
        candidate: None,
        batches: BTreeMap::new(),
        dry_run: false,
        verbose: Verbose::new(false),
    };
    let mut result = record(GithubState::Pending);
    work.package(
        publication.publication.packages.first().unwrap(),
        forge.tag(&result.tag).unwrap(),
        &mut result,
    )
    .unwrap_err();
    assert_eq!(result.source, Some("c".repeat(40)));
    assert!(result.recovery_source.is_none());
    assert!(work.batches.is_empty());
    assert_eq!(
        forge.tags.borrow().get("tool-v1.0.0"),
        Some(&"c".repeat(40))
    );
    assert_eq!(
        *forge.calls.borrow(),
        ["tag:tool-v1.0.0", "create:tool-v1.0.0", "tag:tool-v1.0.0"]
    );
}

fn publication() -> PublicationManifest {
    PublicationManifest::new(
        serde_json::from_value(json!({
            "schema_version":1,"tool_version":"1.0.0","source":"a".repeat(40),
            "workspace_manifest":"Cargo.toml","config_path":".cargo/release_plan.toml",
            "configuration":{"schema-version":1,"repository":"example/tool","release-branch":"main",
                "targets":["x86_64-unknown-linux-gnu"]},
            "packages":[{"name":"tool","version":"1.0.0","manifest":"Cargo.toml",
                "binary":{"name":"tool-bin","targets":["x86_64-unknown-linux-gnu"]}}]
        }))
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn missing_tag_uses_equivalent_candidate_and_emits_linked_binary_work() {
    let publication = publication();
    let forge = FakeForge::default();
    let mut work = Reconciliation {
        github: &forge,
        publication: &publication,
        load_candidate: || Ok(candidate("1.0.0", true)),
        retry_pause: |_| {},
        candidate: None,
        batches: BTreeMap::new(),
        dry_run: false,
        verbose: Verbose::new(false),
    };
    let mut result = record(GithubState::Pending);
    work.package(
        publication.publication.packages.first().unwrap(),
        forge.tag(&result.tag).unwrap(),
        &mut result,
    )
    .unwrap();
    assert_eq!(result.state, GithubState::Complete);
    assert_eq!(result.source, Some("b".repeat(40)));
    assert!(result.recovery_source.is_none());
    assert_eq!(result.observed_version.as_deref(), Some("1.0.0"));
    let batch = work.batches.values().next().unwrap();
    assert_eq!(batch.publication_id, publication.id);
    assert_eq!(batch.binaries.first().unwrap().source_sha, "b".repeat(40));
    assert_eq!(
        *forge.calls.borrow(),
        [
            "tag:tool-v1.0.0",
            "create:tool-v1.0.0",
            "tag:tool-v1.0.0",
            "release:tool-v1.0.0",
            "assets"
        ]
    );
}

#[test]
fn existing_tag_bypasses_current_candidate_and_records_only_consulted_evidence() {
    let publication = publication();
    let forge = FakeForge::default();
    forge
        .tags
        .borrow_mut()
        .insert("tool-v1.0.0".to_owned(), "a".repeat(40));
    let mut work = Reconciliation {
        github: &forge,
        publication: &publication,
        load_candidate: || -> Result<Candidate, AppError> {
            panic!("existing tags do not select a new candidate")
        },
        retry_pause: |_| {},
        candidate: Some(candidate("2.0.0", true)),
        batches: BTreeMap::new(),
        dry_run: false,
        verbose: Verbose::new(false),
    };
    let mut result = record(GithubState::Pending);
    work.package(
        publication.publication.packages.first().unwrap(),
        forge.tag(&result.tag).unwrap(),
        &mut result,
    )
    .unwrap();
    assert_eq!(result.state, GithubState::Complete);
    assert_eq!(result.source, Some("a".repeat(40)));
    assert!(result.observed_version.is_none());
    assert!(result.recovery_source.is_none());
    assert!(
        !forge
            .calls
            .borrow()
            .iter()
            .any(|call| call.starts_with("create:"))
    );
}

#[test]
fn incompatible_candidate_retains_manual_recovery_without_writes() {
    let publication = publication();
    for (version, unchanged) in [("1.0.1", true), ("1.0.0", false)] {
        let forge = FakeForge::default();
        let mut work = Reconciliation {
            github: &forge,
            publication: &publication,
            load_candidate: || Ok(candidate(version, unchanged)),
            retry_pause: |_| {},
            candidate: None,
            batches: BTreeMap::new(),
            dry_run: false,
            verbose: Verbose::new(false),
        };
        let mut result = record(GithubState::Pending);
        work.package(
            publication.publication.packages.first().unwrap(),
            forge.tag(&result.tag).unwrap(),
            &mut result,
        )
        .unwrap_err();
        assert_eq!(result.recovery_source, Some("a".repeat(40)));
        assert_eq!(result.observed_version.as_deref(), Some(version));
        assert_eq!(*forge.calls.borrow(), ["tag:tool-v1.0.0"]);
        assert!(work.batches.is_empty());
    }
}

#[test]
fn dry_run_never_creates_missing_tags_or_releases() {
    let publication = publication();
    for tagged in [false, true] {
        let forge = FakeForge::default();
        if tagged {
            forge
                .tags
                .borrow_mut()
                .insert("tool-v1.0.0".to_owned(), "a".repeat(40));
        }
        let mut work = Reconciliation {
            github: &forge,
            publication: &publication,
            load_candidate: || Ok(candidate("1.0.0", true)),
            retry_pause: |_| {},
            candidate: None,
            batches: BTreeMap::new(),
            dry_run: true,
            verbose: Verbose::new(false),
        };
        let mut result = record(GithubState::Pending);
        work.package(
            publication.publication.packages.first().unwrap(),
            forge.tag(&result.tag).unwrap(),
            &mut result,
        )
        .unwrap();
        assert_eq!(
            result.state,
            if tagged {
                GithubState::WouldCreateRelease
            } else {
                GithubState::WouldCreateTag
            }
        );
        assert!(
            !forge
                .calls
                .borrow()
                .iter()
                .any(|call| call.starts_with("create:"))
        );
        assert!(work.batches.is_empty());
        assert!(result.recovery_source.is_none());
    }
}

#[test]
fn tag_retries_refresh_source_and_stop_at_the_bounded_attempt_count() {
    let publication = publication();
    for failures in [1, 3] {
        let forge = FakeForge {
            failures: Cell::new(failures),
            ..FakeForge::default()
        };
        let loads = Cell::new(0);
        let mut work = Reconciliation {
            github: &forge,
            publication: &publication,
            load_candidate: || {
                loads.set(loads.get() + 1);
                Ok(candidate("1.0.0", true))
            },
            retry_pause: |_| {},
            candidate: None,
            batches: BTreeMap::new(),
            dry_run: false,
            verbose: Verbose::new(false),
        };
        let mut result = record(GithubState::Pending);
        let attempted = work.package(
            publication.publication.packages.first().unwrap(),
            forge.tag(&result.tag).unwrap(),
            &mut result,
        );
        assert_eq!(attempted.is_ok(), failures == 1);
        assert_eq!(loads.get(), if failures == 1 { 2 } else { 3 });
    }
}

#[test]
fn moved_tag_does_not_replace_verified_release_or_batch_source() {
    let publication = publication();
    let forge = FakeForge::default();
    // The caller verified one commit before the externally owned ref moved.
    let verified_source = "a".repeat(40);
    forge
        .tags
        .borrow_mut()
        .insert("tool-v1.0.0".to_owned(), "c".repeat(40));
    let mut work = Reconciliation {
        github: &forge,
        publication: &publication,
        load_candidate: || -> Result<Candidate, AppError> {
            panic!("verified existing tags do not select a candidate")
        },
        retry_pause: |_| {},
        candidate: None,
        batches: BTreeMap::new(),
        dry_run: false,
        verbose: Verbose::new(false),
    };
    let mut result = record(GithubState::Pending);
    work.package(
        publication.publication.packages.first().unwrap(),
        Some(verified_source.clone()),
        &mut result,
    )
    .unwrap();
    assert_eq!(result.source.as_ref(), Some(&verified_source));
    assert_eq!(
        forge.release_sources.borrow().as_slice(),
        slice::from_ref(&verified_source)
    );
    let batch = work.batches.values().next().unwrap();
    assert_eq!(batch.binaries.first().unwrap().source_sha, verified_source);
}
