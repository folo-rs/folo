#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns reconciliation; its child file contains only unit tests."
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, thread};

use ohno::AppError;

use crate::publication::artifact::{require_separate_outputs, write_outcome};
use crate::publication::binaries::Binary;
use crate::publication::context::WorkflowRun;
use crate::publication::github::candidate::{Candidate, tag_workspace};
use crate::publication::github::client::{Forge, GithubResourceMissing};
use crate::publication::github::outcome::GITHUB_OUTCOME_SCHEMA_VERSION;
use crate::publication::github::{
    BatchArtifact, Github, GithubOutcome, GithubPackage, GithubState,
    PLATFORM_BATCH_SCHEMA_VERSION, PlatformBatch,
};
use crate::publication::manifest::{Package, PublicationManifest};
use crate::publication::registry::RegistryClient;
use crate::publication::source::verify_source;
use crate::{PublicationOutput, ReadFileError, WriteFileError};

#[cfg_attr(test, mutants::skip)] // Artifact and transport wiring; outcome decisions are unit-tested.
pub fn publish(
    publication_path: &Path,
    manifest_path: &Path,
    output: &Path,
    batches: &Path,
    dry_run: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    require_separate_outputs(output, batches)?;
    for path in [output, batches] {
        if path
            .try_exists()
            .map_err(|error| ReadFileError::caused_by(path, error))?
        {
            return Err(ReconciliationDestinationExists::new(path.to_path_buf()).into());
        }
    }
    let publication = PublicationManifest::read(publication_path)?;
    let mut outcome = GithubOutcome {
        schema_version: GITHUB_OUTCOME_SCHEMA_VERSION,
        publication_id: publication.id.clone(),
        phase: "github".to_owned(),
        dry_run,
        complete: false,
        packages: Vec::new(),
        batches: Vec::new(),
        planned_targets: Vec::new(),
        errors: Vec::new(),
        github: WorkflowRun::capture()?,
    };
    let result = reconcile(
        &publication,
        manifest_path,
        batches,
        &mut outcome,
        diagnostics,
    );
    if let Err(error) = result {
        diagnostics.line(format_args!("{error}"));
        outcome.errors.push(
            "GitHub reconciliation did not complete; inspect command diagnostics.".to_owned(),
        );
    }
    let passed = outcome.passed(publication.publication.packages.len());
    outcome.complete = !dry_run && passed;
    write_outcome(output, &outcome)?;
    Ok((
        passed,
        format!(
            "GitHub reconciliation {} for {}; outcome: {}.",
            if passed { "completed" } else { "failed" },
            publication.id,
            output.display()
        ),
    ))
}

#[cfg_attr(test, mutants::skip)] // Production client acquisition and verified-source wiring.
fn reconcile(
    publication: &PublicationManifest,
    manifest: &Path,
    batches_path: &Path,
    outcome: &mut GithubOutcome,
    diagnostics: &PublicationOutput,
) -> Result<(), AppError> {
    if publication.publication.packages.is_empty() {
        verify_source(publication, manifest)?;
        return Ok(());
    }
    let registry = RegistryClient::new(diagnostics.clone())?;
    let github = Github::new(
        publication.publication.configuration.repository(),
        diagnostics,
    )?;
    reconcile_with(
        publication,
        manifest,
        batches_path,
        outcome,
        diagnostics,
        &registry,
        &github,
    )
}

/// Shared reconciliation boundary with explicit transports for loopback protocol tests.
#[cfg_attr(test, mutants::skip)] // Real source/registry acquisition; Reconciliation has in-process fakes.
pub fn reconcile_with(
    publication: &PublicationManifest,
    manifest: &Path,
    batches_path: &Path,
    outcome: &mut GithubOutcome,
    diagnostics: &PublicationOutput,
    registry: &RegistryClient,
    github: &Github,
) -> Result<(), AppError> {
    let repository = verify_source(publication, manifest)?.repository;
    // Registry availability is a whole-manifest prerequisite for any GitHub mutation.
    // Do not fold this gate into the later best-effort package loop.
    let mut snapshots = BTreeMap::new();
    for package in &publication.publication.packages {
        if !registry.contains(&package.name, &package.version)? {
            return Err(RegistryPrerequisiteMissing::new(
                package.name.clone(),
                package.version.clone(),
            )
            .into());
        }
    }
    let mut work = Reconciliation {
        github,
        publication,
        load_candidate: || Candidate::create(repository.root(), publication, diagnostics),
        retry_pause: thread::sleep,
        candidate: None,
        batches: BTreeMap::new(),
        dry_run: outcome.dry_run,
        diagnostics,
    };
    for package in &publication.publication.packages {
        let tag = format!("{}-v{}", package.name, package.version);
        let mut record = GithubPackage {
            name: package.name.clone(),
            version: package.version.clone(),
            tag,
            state: GithubState::Pending,
            source: None,
            recovery_source: None,
            observed_version: None,
        };
        // Existing refs are never changed. Verify only their historical package identity,
        // not current release policy, so an operator-created old-version tag can recover.
        let existing = github.tag(&record.tag);
        let identity: Result<(), AppError> = match &existing {
            Ok(Some(source)) => {
                let snapshot = snapshots.entry(source.clone()).or_insert_with(|| {
                    tag_workspace(repository.root(), publication, source, diagnostics)
                });
                match snapshot {
                    Ok(workspace)
                        if workspace.contains_release(
                            &package.name,
                            &package.version,
                            package.binary.as_ref().map(|binary| binary.name.as_str()),
                        ) =>
                    {
                        Ok(())
                    }
                    Ok(_) => {
                        Err(TagPackageMismatch::new(record.tag.clone(), source.clone()).into())
                    }
                    Err(error) => {
                        diagnostics.line(format_args!("Tag snapshot {source}: {error}"));
                        Err(TagPackageMismatch::new(record.tag.clone(), source.clone()).into())
                    }
                }
            }
            Ok(None) => Ok(()),
            Err(_) => Err(TagObservationFailed::new(record.tag.clone()).into()),
        };
        if let Err(error) = identity {
            diagnostics.line(format_args!("{}: {error}", record.tag));
            if let Err(error) = &existing {
                diagnostics.line(format_args!("{error}"));
            }
            record.state = GithubState::Failed;
            record.source = existing.ok().flatten();
            outcome.errors.push(format!("Could not verify existing tag {} for the requested package release; inspect command diagnostics before retrying.",record.tag));
            outcome.packages.push(record);
            continue;
        }
        if let Err(error) = work.package(package, existing?, &mut record) {
            diagnostics.line(format_args!("{}: {error}", record.tag));
            let tag = &record.tag;
            outcome.errors.push(match &record.recovery_source {
                    Some(source) => {
                        let reason = error.find_source::<CandidateMismatch>().map_or_else(
                            || "Tag creation could not be confirmed; inspect the command diagnostics.".to_owned(),
                            |failure| format!("The release branch has {}@{}, not release-equivalent {}@{}.",
                                failure.package, failure.observed, failure.package, failure.requested),
                        );
                        format!("Cannot create {tag}. {reason} Verify and create this missing tag at {source} using operator rights, then retry the original failed workflow. Do not move an existing tag.")
                    },
                    None => format!("GitHub reconciliation failed for {tag}; inspect the command diagnostics before retrying."),
                });
            record.state = GithubState::Failed;
        }
        outcome.packages.push(record);
    }
    outcome.planned_targets = work.batches.keys().cloned().collect();
    if !outcome.dry_run {
        fs::create_dir_all(batches_path)
            .map_err(|error| WriteFileError::caused_by(batches_path, error))?;
        for (target, batch) in work.batches {
            let batch = batch.seal()?;
            let filename = format!("{target}.json");
            write_outcome(&batches_path.join(&filename), &batch)?;
            outcome.batches.push(BatchArtifact {
                target,
                path: filename,
                batch_id: batch.batch_id,
            });
        }
    }
    Ok(())
}

/// Shares one verified release tip and the resulting native batches across package requests.
struct Reconciliation<'a, F, C> {
    github: &'a F,
    publication: &'a PublicationManifest,
    load_candidate: C,
    retry_pause: fn(Duration),
    candidate: Option<Candidate>,
    batches: BTreeMap<String, PlatformBatch>,
    dry_run: bool,
    diagnostics: &'a PublicationOutput,
}

impl<F: Forge, C: FnMut() -> Result<Candidate, AppError>> Reconciliation<'_, F, C> {
    fn package(
        &mut self,
        package: &Package,
        verified_source: Option<String>,
        record: &mut GithubPackage,
    ) -> Result<(), AppError> {
        // Keep the identity already verified by the caller; another lookup could substitute
        // an unverified source if the ref changes before release or batch creation.
        let source = match verified_source {
            Some(source) => source,
            None => {
                record.recovery_source = Some(self.publication.publication.source.clone());
                let Some(source) = self.create_tag(package, record)? else {
                    record.recovery_source = None;
                    record.state = GithubState::WouldCreateTag;
                    return Ok(());
                };
                record.recovery_source = None;
                source
            }
        };
        record.source = Some(source.clone());
        let Some(binary) = &package.binary else {
            record.state = GithubState::Complete;
            return Ok(());
        };
        let (assets, state) = match self.github.ensure_release(
            &record.tag,
            &package.version,
            &source,
            self.dry_run,
        )? {
            Some(release) => (self.github.assets(&release)?, GithubState::Complete),
            // An established tag with no release has no complete asset pairs. Dry runs can
            // report those targets without emitting authoritative batch files or writing GitHub.
            None => (Vec::new(), GithubState::WouldCreateRelease),
        };
        for target in &binary.targets {
            let binary = Binary {
                name: package.name.clone(),
                bin: binary.name.clone(),
                version: package.version.clone(),
                tag: record.tag.clone(),
                source_sha: source.clone(),
            };
            if !binary.complete(target.triple(), &assets) {
                self.batches
                    .entry(target.triple().to_owned())
                    .or_insert_with(|| PlatformBatch {
                        schema_version: PLATFORM_BATCH_SCHEMA_VERSION,
                        publication_id: self.publication.id.clone(),
                        repository: self
                            .publication
                            .publication
                            .configuration
                            .repository()
                            .to_owned(),
                        target: target.triple().to_owned(),
                        binaries: Vec::new(),
                        batch_id: String::new(),
                    })
                    .binaries
                    .push(binary);
            }
        }
        record.state = state;
        Ok(())
    }

    fn create_tag(
        &mut self,
        package: &Package,
        record: &mut GithubPackage,
    ) -> Result<Option<String>, AppError> {
        // Ref writes can race branch movement or lose their response. Each retry revalidates
        // fresh source rather than retrying indefinitely against a stale candidate.
        // The attempt/pause envelope is a deliberately small engineering allowance for
        // transient races before an operator handoff, not a GitHub propagation guarantee.
        const ATTEMPTS: usize = 3;
        let tag = &record.tag;
        for attempt in 1..=ATTEMPTS {
            if self.candidate.is_none() {
                self.candidate = Some((self.load_candidate)()?);
            }
            let candidate = self
                .candidate
                .as_ref()
                .expect("candidate was prepared above");
            record.observed_version = candidate
                .packages
                .get(&package.name)
                .map(|package| package.version.clone());
            if !candidate.supports(package) {
                let actual = candidate
                    .packages
                    .get(&package.name)
                    .map_or("absent", |package| package.version.as_str());
                return Err(CandidateMismatch::new(
                    package.name.clone(),
                    package.version.clone(),
                    actual.to_owned(),
                )
                .into());
            }
            self.diagnostics.notes().note(|| format!(
                "{tag} is absent; candidate {} retains its requested version and released content.", candidate.source
            ));
            if self.dry_run {
                return Ok(None);
            }
            let created = self.github.create_tag(tag, &candidate.source);
            if let Err(error) = &created {
                self.diagnostics.line(format_args!(
                    "Tag creation attempt {attempt} failed: {error}"
                ));
            }
            let observed = match self.github.tag(tag) {
                Ok(observed) => observed,
                Err(confirmation) => {
                    return Err(match created {
                        Ok(()) => confirmation,
                        Err(operation) => {
                            TagConfirmationFailed::caused_by(tag.clone(), confirmation, operation)
                                .into()
                        }
                    });
                }
            };
            if let Some(source) = observed {
                if source != candidate.source {
                    record.source = Some(source.clone());
                    record.recovery_source = None;
                    return Err(
                        CompetingTag::new(tag.clone(), source, candidate.source.clone()).into(),
                    );
                }
                return Ok(Some(source));
            }
            if attempt == ATTEMPTS {
                created?;
                return Err(GithubResourceMissing::new(tag.to_owned()).into());
            }
            self.candidate = None;
            // A short operational pause avoids immediately repeating a rejected ref write.
            // This candidate-refresh policy is independent of native asset-upload retries.
            (self.retry_pause)(Duration::from_secs(5));
        }
        unreachable!("the last tag-creation attempt returns")
    }
}

/// A missing-tag candidate cannot satisfy the immutable requested release.
#[ohno::error]
#[display("release branch has {package}@{observed}, not release-equivalent {package}@{requested}")]
struct CandidateMismatch {
    package: String,
    requested: String,
    observed: String,
}

/// Existing tags are preserved when their historical package identity cannot be established.
#[ohno::error]
#[display("tag {tag} at {source} does not establish the requested package identity")]
struct TagPackageMismatch {
    tag: String,
    source: String,
}

/// An existing destination must not replace this or an earlier attempt's evidence.
#[ohno::error]
#[display("GitHub reconciliation destination must be new: {}", path.display())]
struct ReconciliationDestinationExists {
    path: PathBuf,
}

/// All exact versions must be available before any forge mutation.
#[ohno::error]
#[display("{package}@{version} is not available in the registry")]
struct RegistryPrerequisiteMissing {
    package: String,
    version: String,
}

#[ohno::error]
#[display("cannot determine existing tag identity for {tag}")]
struct TagObservationFailed {
    tag: String,
}

/// A competing creator cannot replace the source already verified for this attempt.
#[ohno::error]
#[display("tag {tag} appeared at {observed}, not verified candidate {expected}; preserve the ref")]
struct CompetingTag {
    tag: String,
    observed: String,
    expected: String,
}

/// A failed ref write remains relevant when its follow-up observation also fails.
#[ohno::error]
#[display("tag {tag} creation confirmation also failed: {confirmation}")]
struct TagConfirmationFailed {
    tag: String,
    confirmation: AppError,
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
