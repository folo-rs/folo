use std::collections::BTreeMap;
use std::path::Path;

use crp_publication::publication::candidate::{CandidateRequest, verify};
use ohno::AppError;

use crate::git_fixture::Repository;

/// Supplies real Cargo source for the candidate verifier, using the boundary suite's Git fixture.
pub(crate) struct Fixture {
    repository: Repository,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let fixture = Self {
            repository: Repository::new(),
        };
        fixture.git(&["checkout", "-b", "main"]);
        fixture.write(".gitignore", "target/\n");
        fixture.write(".github/workflows/release.yml", "name: fixture\n");
        fixture.write_workspace("MIT");
        fixture.write_package("1.0.0");
        fixture.write("packages/widget/src/lib.rs", "pub fn value() -> u8 { 1 }\n");
        fixture.commit("initial release");
        fixture
    }

    pub(crate) fn root(&self) -> &Path {
        self.repository.path()
    }

    pub(crate) fn write_workspace(&self, license: &str) {
        self.write(
            "Cargo.toml",
            &format!(
                "[workspace]\nmembers = [\"packages/*\"]\nresolver = \"2\"\n\
                 [workspace.package]\nlicense = \"{license}\"\n"
            ),
        );
    }

    pub(crate) fn write_package(&self, version: &str) {
        self.write(
            "packages/widget/Cargo.toml",
            &format!(
                "[package]\nname = \"widget\"\nversion = \"{version}\"\nedition = \"2021\"\n\
                 license.workspace = true\ninclude = [\"src/**\"]\n"
            ),
        );
        self.write(
            "Cargo.lock",
            &format!("version = 4\n\n[[package]]\nname = \"widget\"\nversion = \"{version}\"\n"),
        );
    }

    pub(crate) fn write(&self, relative: &str, text: &str) {
        self.repository.write(relative, text.as_bytes());
    }

    pub(crate) fn commit(&self, message: &str) -> String {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-m", message]);
        self.head()
    }

    pub(crate) fn head(&self) -> String {
        self.git(&["rev-parse", "HEAD"]).trim().to_owned()
    }

    pub(crate) fn git(&self, arguments: &[&str]) -> String {
        self.repository.command(arguments)
    }

    pub(crate) fn verify(
        &self,
        commit: &str,
        release_line: &str,
        version: &str,
    ) -> Result<String, AppError> {
        verify(
            &CandidateRequest {
                manifest_path: self.root().join("Cargo.toml"),
                commit: commit.to_owned(),
                release_line: release_line.to_owned(),
                packages: BTreeMap::from([("widget".to_owned(), version.parse().unwrap())]),
                verbose: true,
            },
            crp_diag::Verbose::new(true, &crp_diag::Discard),
        )
    }
}
