#![allow(
    clippy::self_named_module_files,
    reason = "The subject module owns reconciliation; its child file contains only unit tests."
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;
use std::{fs, thread};

use ohno::AppError;

use crate::PublicationOutput;
use crate::publication::binaries::Binary;
use crate::publication::context::WorkflowRun;
use crate::publication::github::candidate::{Candidate, verify_tag_source};
use crate::publication::github::client::{Forge, GithubResourceMissing};
use crate::publication::github::{
    BatchArtifact, Github, GithubOutcome, GithubPackage, GithubState, PlatformBatch,
};
use crate::publication::manifest::{InvalidManifest, Package, PublicationManifest};
use crate::publication::registry::{RegistryClient, verify_source, write_outcome};

#[cfg_attr(test, mutants::skip)] // Artifact and transport wiring; outcome decisions are unit-tested.
pub fn publish(
    publication_path: &Path,
    manifest_path: &Path,
    output: &Path,
    batches: &Path,
    dry_run: bool,
    diagnostics: &PublicationOutput,
) -> Result<(bool, String), AppError> {
    if output.try_exists()? || batches.try_exists()? {
        return Err(InvalidManifest::new(
            "GitHub reconciliation requires new outcome and batch destinations".to_owned(),
        )
        .into());
    }
    let publication = PublicationManifest::read(publication_path)?;
    let mut outcome = GithubOutcome {
        schema_version: 1,
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

/// Shared reconciliation boundary with explicit transports for native protocol tests.
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
    let repository = verify_source(publication, manifest)?;
    for package in &publication.publication.packages {
        if !registry.contains(&package.name, &package.version)? {
            return Err(InvalidManifest::new(format!(
                "{}@{} is not available in the registry",
                package.name, package.version
            ))
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
        let identity = match &existing {
            Ok(Some(source)) => {
                verify_tag_source(repository.root(), publication, package, source, diagnostics)
            }
            Ok(None) => Ok(()),
            Err(_) => Err(InvalidManifest::new(
                "cannot determine existing tag identity".to_owned(),
            )
            .into()),
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
                    Some(source) => format!("Cannot create {tag}. Verify and create this missing tag at {source} using operator rights, then retry the original failed workflow. Do not move an existing tag."),
                    None => format!("GitHub reconciliation failed for {tag}; inspect the command diagnostics before retrying."),
                });
            record.state = GithubState::Failed;
        }
        outcome.packages.push(record);
    }
    outcome.planned_targets = work.batches.keys().cloned().collect();
    if !outcome.dry_run {
        fs::create_dir_all(batches_path)?;
        for (target, batch) in &work.batches {
            let batch = batch.clone().seal()?;
            let filename = format!("{target}.json");
            write_outcome(&batches_path.join(&filename), &batch)?;
            outcome.batches.push(BatchArtifact {
                target: target.clone(),
                path: filename,
                batch_id: batch.batch_id,
            });
        }
    }
    if let Some(mut candidate) = work.candidate.take() {
        candidate.finish()?;
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
        let Some(release) =
            self.github
                .ensure_release(&record.tag, &package.version, &source, self.dry_run)?
        else {
            record.state = GithubState::WouldCreateRelease;
            return Ok(());
        };
        let assets = self.github.assets(&release)?;
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
                        schema_version: 1,
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
        record.state = GithubState::Complete;
        Ok(())
    }

    fn create_tag(
        &mut self,
        package: &Package,
        record: &mut GithubPackage,
    ) -> Result<Option<String>, AppError> {
        // Ref writes can race branch movement or lose their response. Each retry revalidates
        // fresh source rather than retrying indefinitely against a stale candidate.
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
                return Err(InvalidManifest::new(format!(
                    "current release branch has {}@{actual}, not release-equivalent {}@{}",
                    package.name, package.name, package.version
                ))
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
            if let Some(source) = self.github.tag(tag)? {
                if source != candidate.source {
                    record.source = Some(source.clone());
                    record.recovery_source = None;
                    return Err(InvalidManifest::new(format!(
                        "tag {tag} appeared at {source}, not verified candidate {}; preserve the existing ref and inspect the competing creation",
                        candidate.source
                    )).into());
                }
                return Ok(Some(source));
            }
            if attempt == ATTEMPTS {
                created?;
                return Err(GithubResourceMissing::new(tag.to_owned()).into());
            }
            if let Some(mut candidate) = self.candidate.take() {
                candidate.finish()?;
            }
            // Match the native GitHub operation's short infrastructure retry window.
            (self.retry_pause)(Duration::from_secs(5));
        }
        unreachable!("the last tag-creation attempt returns")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests;
