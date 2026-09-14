// Captured repository inputs and the exact resolved manifest/lockfile writes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use ohno::AppError;
use serde::{Deserialize, Serialize};
use toml_edit::{Item, TableLike};

use crate::command::hash_bytes;
use crate::manifest::{for_each_dependency_table, parse_document};
use crate::metadata::load_tracked_work_tree;
use crate::plan::{PlanFile, PlanStage, SCHEMA_VERSION, resolve_plan};
use crate::verbose::Verbose;
use crate::{ParsePlanError, ReadFileError, UnsupportedPlanSchemaError, WriteFileError};

/// Repository facts frozen before any resolver-driven release decisions.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Inputs {
    root: PathBuf,
    pub(crate) manifest: PathBuf,
    pub(crate) head: String,
    pub(crate) base: String,
    base_revision: String,
    index: String,
    pub(crate) paths: BTreeSet<PathBuf>,
    digest: String,
}

impl Inputs {
    pub(crate) fn capture(manifest: &Path, base: Option<&str>) -> Result<Self, AppError> {
        // Cargo preserves the supplied path spelling, including Windows short names.
        // Normalize the entry point before discovering any paths that will be rebased.
        let manifest = canonical(manifest)?;
        let (work_tree, git) = load_tracked_work_tree(&manifest)?;
        let root = canonical(git.root())?;
        let manifest = relative(&root, &manifest)?;
        let base_revision = match base {
            Some(base) => base.to_owned(),
            None => git.default_base()?.revision().to_owned(),
        };
        let mut paths: BTreeSet<PathBuf> =
            work_tree.index.paths.iter().map(PathBuf::from).collect();
        paths.insert(relative(
            &root,
            &work_tree.workspace_root.join("Cargo.lock"),
        )?);
        for directory in work_tree
            .workspace_root
            .ancestors()
            .take_while(|directory| directory.starts_with(&root))
        {
            paths.insert(relative(&root, &directory.join(".cargo/config"))?);
            paths.insert(relative(&root, &directory.join(".cargo/config.toml"))?);
        }
        for manifest in &work_tree.member_manifests {
            paths.insert(relative(&root, manifest)?);
            let directory = manifest
                .parent()
                .expect("a manifest has a parent directory");
            collect_sources(&root, &directory.join("src"), &mut paths)?;
            paths.insert(relative(&root, &directory.join("build.rs"))?);
        }
        capture_path_dependencies(
            &root,
            work_tree
                .member_manifests
                .iter()
                .chain([&work_tree.workspace_root.join("Cargo.toml")]),
            &mut paths,
        )?;
        let digest = fingerprint(&root, &paths, &BTreeMap::new())?;
        let (head, base) = git.head_and_base(&base_revision)?;
        Ok(Self {
            root,
            manifest,
            head,
            base,
            base_revision,
            index: work_tree.index.raw,
            paths,
            digest,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn index(&self) -> &str {
        &self.index
    }

    /// Verifies the same captured input set in a relocated final workspace.
    pub(crate) fn verify_candidate(
        &self,
        manifest: &Path,
        final_digest: &str,
    ) -> Result<(), AppError> {
        let current = Self::capture(manifest, Some(&self.base)).map_err(StaleInputs::caused_by)?;
        self.compare_candidate(&current, final_digest)
    }

    fn compare_candidate(&self, current: &Self, final_digest: &str) -> Result<(), AppError> {
        if current.manifest != self.manifest
            || current.head != self.head
            || current.base != self.base
            || current.index != self.index
            || current.paths != self.paths
            || current.digest != final_digest
        {
            return Err(StaleInputs::new().into());
        }
        Ok(())
    }

    /// Accepts only the complete initial state or the complete captured final state.
    pub(crate) fn verify(
        &self,
        manifest: &Path,
        final_digest: Option<&str>,
    ) -> Result<bool, AppError> {
        let current =
            Self::capture(manifest, Some(&self.base_revision)).map_err(StaleInputs::caused_by)?;
        self.compare(&current, final_digest)
    }

    fn compare(&self, current: &Self, final_digest: Option<&str>) -> Result<bool, AppError> {
        if current.root != self.root
            || current.manifest != self.manifest
            || current.head != self.head
            || current.base != self.base
            || current.index != self.index
            || current.paths != self.paths
        {
            return Err(StaleInputs::new().into());
        }
        if current.digest == self.digest {
            return Ok(false);
        }
        if final_digest == Some(current.digest.as_str()) {
            return Ok(true);
        }
        Err(StaleInputs::new().into())
    }

    pub(crate) fn final_digest(&self, files: &[Artifact]) -> Result<String, AppError> {
        let mut seen = BTreeSet::new();
        for file in files {
            if !self.paths.contains(&file.path)
                || !matches!(
                    file.path.file_name().and_then(|name| name.to_str()),
                    Some("Cargo.toml" | "Cargo.lock")
                )
                || !seen.insert(&file.path)
            {
                return Err(ResolutionRequired::new().into());
            }
        }
        let replacements = files
            .iter()
            .map(|file| (file.path.clone(), file.contents.as_bytes().to_vec()))
            .collect();
        fingerprint(&self.root, &self.paths, &replacements)
    }
}

fn capture_path_dependencies<'a>(
    root: &Path,
    manifests: impl IntoIterator<Item = &'a PathBuf>,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), AppError> {
    let mut pending: BTreeSet<PathBuf> = manifests.into_iter().cloned().collect();
    let mut visited = BTreeSet::new();
    while let Some(manifest) = pending.pop_first() {
        if !visited.insert(manifest.clone()) {
            continue;
        }
        paths.insert(relative(root, &manifest)?);
        let text = fs::read_to_string(&manifest)
            .map_err(|error| ReadFileError::caused_by(&manifest, error))?;
        let document = parse_document(&manifest, &text)?;
        let mut dependencies = Vec::new();
        for_each_dependency_table(document.as_table(), &mut |_, table| {
            dependency_paths(table, &mut dependencies);
        });
        if let Some(workspace) = document.get("workspace").and_then(Item::as_table_like) {
            for_each_dependency_table(workspace, &mut |_, table| {
                dependency_paths(table, &mut dependencies);
            });
        }
        if let Some(patches) = document.get("patch").and_then(Item::as_table_like) {
            for (_, patch) in patches.iter() {
                if let Some(table) = patch.as_table_like() {
                    dependency_paths(table, &mut dependencies);
                }
            }
        }
        if let Some(replacements) = document.get("replace").and_then(Item::as_table_like) {
            dependency_paths(replacements, &mut dependencies);
        }
        for dependency in dependencies {
            let path = Path::new(&dependency);
            // Absolute paths would keep pointing into the live workspace from a disposable clone.
            if path.is_absolute() {
                return Err(UnsupportedInput::new(path).into());
            }
            let directory = manifest
                .parent()
                .expect("a manifest has a parent")
                .join(path);
            let directory = canonical(&directory)?;
            relative(root, &directory)?;
            collect_sources(root, &directory.join("src"), paths)?;
            pending.insert(directory.join("Cargo.toml"));
        }
    }
    Ok(())
}

fn dependency_paths(table: &dyn TableLike, paths: &mut Vec<String>) {
    for (_, dependency) in table.iter() {
        if let Some(path) = dependency
            .as_table_like()
            .and_then(|dependency| dependency.get("path"))
            .and_then(Item::as_str)
        {
            paths.push(path.to_owned());
        }
    }
}

/// The complete resolved state embedded in the explicit plan for application.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ResolvedState {
    pub(crate) inputs: Inputs,
    pub(crate) files: Vec<Artifact>,
    pub(crate) final_digest: String,
    pub(crate) versions: BTreeMap<String, String>,
    pub(crate) evidence_manifest_path: PathBuf,
}

impl ResolvedState {
    pub(crate) fn verify_candidate(&self, manifest: &Path) -> Result<(), AppError> {
        let manifest = canonical(manifest)?;
        if manifest != canonical(&self.evidence_manifest_path)?
            || manifest == canonical(&self.inputs.root.join(&self.inputs.manifest))?
        {
            return Err(WrongEvidenceWorkspace::new().into());
        }
        self.inputs.verify_candidate(&manifest, &self.final_digest)
    }

    fn validate_artifacts(
        &self,
        versions: &BTreeMap<String, String>,
        allowed: &BTreeSet<PathBuf>,
    ) -> Result<(), AppError> {
        if *versions != self.versions {
            return Err(ResolutionRequired::new().into());
        }
        if self.files.iter().any(|file| !allowed.contains(&file.path)) {
            return Err(ResolutionRequired::new().into());
        }
        // final_digest owns captured-path membership and uniqueness; this layer additionally
        // restricts writes to the current workspace's member manifests and lockfile.
        if self.inputs.final_digest(&self.files)? != self.final_digest {
            return Err(ResolutionRequired::new().into());
        }
        Ok(())
    }
}

/// Exact UTF-8 bytes of one resolved manifest or workspace lockfile.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Artifact {
    pub(crate) path: PathBuf,
    pub(crate) contents: String,
}

/// The protocol header is checked before parsing a version-specific artifact body.
#[derive(Deserialize)]
struct ArtifactSchema {
    schema_version: u32,
}

pub(crate) fn run_verify_preview(
    plan_path: &Path,
    manifest: &Path,
    verbose: Verbose,
) -> Result<String, AppError> {
    let plan: PlanFile = read_json(plan_path)?;
    let state = plan.resolved.as_ref().ok_or_else(ResolutionRequired::new)?;
    _ = apply_resolved(
        &plan,
        &state.inputs.root.join(&state.inputs.manifest),
        true,
        verbose,
    )?;
    state.verify_candidate(manifest)?;
    Ok("Compatibility workspace matches the captured source, versions, and lockfile.".to_owned())
}

pub(crate) fn apply_resolved(
    plan: &PlanFile,
    manifest: &Path,
    dry_run: bool,
    verbose: Verbose,
) -> Result<String, AppError> {
    let state = plan.resolved.as_ref().ok_or_else(ResolutionRequired::new)?;
    if plan.schema_version != SCHEMA_VERSION || plan.stage() != PlanStage::Expanded {
        return Err(ResolutionRequired::new().into());
    }
    let manifest = canonical(manifest)?;
    let already_applied = state.inputs.verify(&manifest, Some(&state.final_digest))?;
    let (work_tree, _) = load_tracked_work_tree(&manifest)?;
    let resolved = resolve_plan(
        plan,
        &work_tree.groups,
        &work_tree.target_versions(),
        verbose,
    )?;
    let versions: BTreeMap<String, String> = resolved
        .packages
        .iter()
        .map(|(name, version)| (name.clone(), version.to_string()))
        .collect();
    let allowed: BTreeSet<PathBuf> = work_tree
        .member_manifests
        .iter()
        .chain([&work_tree.workspace_root.join("Cargo.toml")])
        .chain([&work_tree.workspace_root.join("Cargo.lock")])
        .map(|path| relative(state.inputs.root(), path))
        .collect::<Result<_, _>>()?;
    state.validate_artifacts(&versions, &allowed)?;
    if already_applied {
        return Ok("Resolved state is already applied; no files changed.".to_owned());
    }
    if dry_run {
        return Ok(format!(
            "Dry run: would install {} captured files; no Cargo resolution is performed.",
            state.files.len()
        ));
    }
    for file in &state.files {
        let path = state.inputs.root().join(&file.path);
        fs::write(&path, &file.contents)
            .map_err(|error| WriteFileError::caused_by(&path, error))?;
    }
    state.inputs.verify(&manifest, Some(&state.final_digest))?;
    Ok(format!(
        "Installed {} captured files without Cargo resolution.",
        state.files.len()
    ))
}

pub(crate) fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, AppError> {
    let text = fs::read_to_string(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    parse_artifact(path, &text)
}

fn parse_artifact<T: for<'de> Deserialize<'de>>(path: &Path, text: &str) -> Result<T, AppError> {
    // A UTF-8 byte-order mark is encoding metadata, not part of the JSON document.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let schema: ArtifactSchema =
        serde_json::from_str(text).map_err(|error| ParsePlanError::caused_by(path, error))?;
    if schema.schema_version != SCHEMA_VERSION {
        return Err(UnsupportedPlanSchemaError::new(schema.schema_version).into());
    }
    serde_json::from_str(text).map_err(|error| ParsePlanError::caused_by(path, error).into())
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    let json = serde_json::to_string_pretty(value)
        .expect("release artifacts contain only JSON-compatible data");
    fs::write(path, format!("{json}\n"))
        .map_err(|error| WriteFileError::caused_by(path, error).into())
}

pub(crate) fn relative(root: &Path, path: &Path) -> Result<PathBuf, AppError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| UnsupportedInput::caused_by(path, error))?;
    if relative
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(UnsupportedInput::new(path).into());
    }
    Ok(relative.to_path_buf())
}

pub(crate) fn canonical(path: &Path) -> Result<PathBuf, AppError> {
    let canonical =
        fs::canonicalize(path).map_err(|error| ReadFileError::caused_by(path, error))?;
    // Git and Cargo report ordinary Windows paths, not canonicalize's verbatim spelling.
    #[cfg(windows)]
    let canonical = ordinary_windows_path(&canonical);
    Ok(canonical)
}

#[cfg(windows)]
fn ordinary_windows_path(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    match text.strip_prefix(r"\\?\UNC\") {
        Some(path) => PathBuf::from(format!(r"\\{path}")),
        None => PathBuf::from(text.trim_start_matches(r"\\?\")),
    }
}

fn collect_sources(
    root: &Path,
    directory: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> Result<(), AppError> {
    if !directory.exists() {
        return Ok(());
    }
    for entry in
        fs::read_dir(directory).map_err(|error| ReadFileError::caused_by(directory, error))?
    {
        let entry = entry.map_err(|error| ReadFileError::caused_by(directory, error))?;
        let kind = entry
            .file_type()
            .map_err(|error| ReadFileError::caused_by(entry.path(), error))?;
        if kind.is_dir() {
            collect_sources(root, &entry.path(), paths)?;
        } else {
            paths.insert(relative(root, &entry.path())?);
        }
    }
    Ok(())
}

fn fingerprint(
    root: &Path,
    paths: &BTreeSet<PathBuf>,
    replacements: &BTreeMap<PathBuf, Vec<u8>>,
) -> Result<String, AppError> {
    let mut bytes = Vec::new();
    for relative in paths {
        let path = root.join(relative);
        let name = relative.to_string_lossy();
        append_field(&mut bytes, name.as_bytes());
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => Some(metadata),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(ReadFileError::caused_by(&path, error).into()),
        };
        if metadata
            .as_ref()
            .is_some_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(UnsupportedInput::new(&path).into());
        }
        #[cfg(unix)]
        bytes.push(u8::from(metadata.as_ref().is_some_and(|metadata| {
            // Git's executable-file mode records whether any execute bit is present.
            metadata.permissions().mode() & 0o111 != 0
        })));
        let contents = if let Some(replacement) = replacements.get(relative) {
            Some(replacement.clone())
        } else if metadata.is_some() {
            Some(fs::read(&path).map_err(|error| ReadFileError::caused_by(&path, error))?)
        } else {
            None
        };
        bytes.push(u8::from(contents.is_some()));
        if let Some(contents) = contents {
            append_field(&mut bytes, &contents);
        }
    }
    hash_bytes(&bytes, root)
}

fn append_field(bytes: &mut Vec<u8>, field: &[u8]) {
    // Artifact fingerprints use a fixed-width length, independent of the executing binary's
    // pointer width. Length-prefixing also distinguishes adjacent fields with the same bytes.
    let length = u64::try_from(field.len())
        .expect("a slice on a supported Rust target cannot exceed u64::MAX bytes");
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(field);
}

/// A stale source tree must return to preparation rather than widen a captured plan.
#[ohno::error]
#[display("prepared release inputs are stale; regenerate with prepare and preview")]
pub(crate) struct StaleInputs;

/// A plain expansion has not captured the resolution effects required for application.
#[ohno::error]
#[display("apply requires the unchanged resolved plan produced by preview")]
pub(crate) struct ResolutionRequired;

/// Prospective work must remain inside the captured repository.
#[ohno::error]
#[display("cannot capture release input {}; use ordinary files inside the repository", path.display())]
pub(crate) struct UnsupportedInput {
    path: PathBuf,
}

/// Compatibility evidence must come from the recorded final candidate, not the live tree.
#[ohno::error]
#[display("verification requires the evidence_manifest_path recorded in the resolved plan")]
struct WrongEvidenceWorkspace;

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::process::Command;
    use std::sync::LazyLock;

    use serde_json::{Value, json};
    use tempfile::{TempDir, tempdir};

    use super::*;
    use crate::classify::{PackageStatus, classify};
    use crate::prospective::Prospective;

    // A real empty file supports Git for Windows on ARM64, unlike the NUL device.
    // Keep it outside fixtures so it cannot enter their captured or committed inputs.
    static GIT_CONFIG: LazyLock<TempDir> = LazyLock::new(|| {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("config"), "").unwrap();
        directory
    });

    fn git(root: &Path, args: &[&str]) -> String {
        // Capture fixtures need real index/history semantics, but no resolver or preview.
        let output = Command::new("git")
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", GIT_CONFIG.path().join("config"))
            .env_remove("GIT_CONFIG")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env("GIT_TEMPLATE_DIR", "")
            .args([
                "-c",
                "user.email=test@example.invalid",
                "-c",
                "user.name=Test",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "gc.auto=0",
                "-c",
                "core.autocrlf=false",
                "-c",
                "init.templateDir=",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap()
    }

    fn capture_fixture(manifest: &str) -> TempDir {
        let directory = tempdir().unwrap();
        let path = directory.path().join(manifest);
        let package = path.parent().unwrap();
        fs::create_dir_all(package.join("src")).unwrap();
        fs::write(
            &path,
            "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
        )
        .unwrap();
        fs::write(package.join("src/lib.rs"), "pub fn released() {}\n").unwrap();
        fs::write(directory.path().join(".gitignore"), "**/src/generated.rs\n").unwrap();
        fs::create_dir_all(directory.path().join(".cargo")).unwrap();
        fs::write(
            directory.path().join(".cargo/config.toml"),
            "[term]\nquiet = true\n",
        )
        .unwrap();
        git(directory.path(), &["init", "--quiet"]);
        git(directory.path(), &["add", "."]);
        git(
            directory.path(),
            &["commit", "--quiet", "-m", "released fixture"],
        );
        directory
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "captures a real Git and Cargo workspace without resolution"
    )]
    fn capture_includes_local_sources_without_turning_them_into_release_reasons() {
        let directory = capture_fixture("Cargo.toml");
        let manifest = directory.path().join("Cargo.toml");
        let sources = ["src/untracked.rs", "src/generated.rs"];
        for source in sources {
            fs::write(directory.path().join(source), "pub fn local() {}\n").unwrap();
        }
        let inputs = Inputs::capture(&manifest, Some("HEAD")).unwrap();
        for source in sources {
            assert!(inputs.paths.contains(Path::new(source)));
            assert!(!inputs.index.contains(source));
            fs::write(directory.path().join(source), "pub fn changed() {}\n").unwrap();
            let error = inputs.verify(&manifest, None).unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
            fs::write(directory.path().join(source), "pub fn local() {}\n").unwrap();
        }
        let classification = classify(&manifest, Some("HEAD"), Verbose::new(false)).unwrap();
        assert_eq!(
            classification.packages.first().unwrap().status(),
            PackageStatus::Unchanged
        );
    }

    #[test]
    #[cfg_attr(
        miri,
        ignore = "captures a nested Git and Cargo workspace without resolution"
    )]
    fn nested_capture_records_ancestor_configuration_and_the_default_base() {
        let directory = capture_fixture("rust/Cargo.toml");
        let head = git(directory.path(), &["rev-parse", "HEAD"]);
        git(
            directory.path(),
            &["update-ref", "refs/remotes/origin/main", head.trim()],
        );
        let inputs = Inputs::capture(&directory.path().join("rust/Cargo.toml"), None).unwrap();
        assert_eq!(inputs.manifest, Path::new("rust/Cargo.toml"));
        assert_eq!(inputs.base, head.trim());
        assert!(inputs.paths.contains(Path::new(".cargo/config.toml")));
        assert!(inputs.paths.contains(Path::new("rust/.cargo/config.toml")));
        assert!(inputs.paths.contains(Path::new("rust/Cargo.lock")));
    }

    #[test]
    #[cfg(windows)]
    #[cfg_attr(
        miri,
        ignore = "uses Windows short paths and native PowerShell/Git/Cargo"
    )]
    fn short_windows_paths_use_the_same_captured_workspace_identity() {
        let directory = capture_fixture("Cargo.toml");
        // Probe actual short-name availability; capture owns path identity, not resolution.
        let script = directory.path().join("short-path.ps1");
        fs::write(
            &script,
            "# Returns the actual short-name spelling for the capture test's owned directory.\n\
             param([string] $Path)\n\
             Set-StrictMode -Version Latest\n\
             $ErrorActionPreference = 'Stop'\n\
             $PSNativeCommandUseErrorActionPreference = $true\n\
             $filesystem = New-Object -ComObject Scripting.FileSystemObject\n\
             $filesystem.GetFolder($Path).ShortPath\n",
        )
        .unwrap();
        let output = Command::new("pwsh")
            .args(["-NoProfile", "-NonInteractive", "-File"])
            .arg(&script)
            .arg(directory.path())
            .output()
            .unwrap();
        assert!(output.status.success());
        let short_root = PathBuf::from(String::from_utf8(output.stdout).unwrap().trim());
        assert_eq!(
            fs::canonicalize(&short_root).unwrap(),
            fs::canonicalize(directory.path()).unwrap()
        );
        if short_root == directory.path() {
            eprintln!("This volume exposes no distinct short directory name.");
            return;
        }
        let inputs = Inputs::capture(&short_root.join("Cargo.toml"), Some("HEAD")).unwrap();
        assert_eq!(
            fs::canonicalize(inputs.root()).unwrap(),
            fs::canonicalize(directory.path()).unwrap()
        );
        assert_eq!(inputs.manifest, Path::new("Cargo.toml"));
    }

    #[test]
    #[cfg_attr(miri, ignore = "reserves an owned prospective directory")]
    fn occupied_prospective_directory_is_preserved_before_reading_the_source_repository() {
        let directory = tempdir().unwrap();
        let occupied = directory.path().join(".prospective");
        fs::create_dir_all(&occupied).unwrap();
        let marker = occupied.join("keep");
        fs::write(&marker, "another owner").unwrap();
        let error = Prospective::new(directory.path(), &inputs()).err().unwrap();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert_eq!(fs::read_to_string(marker).unwrap(), "another owner");
    }

    fn inputs() -> Inputs {
        Inputs {
            root: PathBuf::from("repository"),
            manifest: PathBuf::from("Cargo.toml"),
            head: "head".to_owned(),
            base: "base".to_owned(),
            base_revision: "main".to_owned(),
            index: "index".to_owned(),
            paths: ["Cargo.toml", "Cargo.lock", "src/lib.rs"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            digest: "initial".to_owned(),
        }
    }

    #[test]
    fn captured_identity_fields_are_required_for_initial_and_final_states() {
        let inputs = inputs();
        let original = serde_json::to_value(&inputs).unwrap();
        for (field, value) in [
            ("root", json!("another-repository")),
            ("manifest", json!("member/Cargo.toml")),
            ("head", json!("another-head")),
            ("base", json!("another-base")),
            ("index", json!("another-index")),
            ("paths", json!(["Cargo.toml"])),
        ] {
            let mut changed = original.clone();
            *changed.get_mut(field).unwrap() = value;
            let current: Inputs = serde_json::from_value(changed).unwrap();
            let error = inputs.compare(&current, Some("final")).unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
            if field != "root" {
                let error = inputs.compare_candidate(&current, "initial").unwrap_err();
                assert!(error.find_source::<StaleInputs>().is_some());
            }
        }
    }

    #[test]
    fn live_inputs_accept_only_initial_or_complete_final_bytes() {
        let inputs = inputs();
        assert!(!inputs.compare(&inputs, None).unwrap());
        assert!(!inputs.compare(&inputs, Some("initial")).unwrap());
        let mut current = inputs.clone();
        current.digest = "final".to_owned();
        assert!(inputs.compare(&current, Some("final")).unwrap());
        for final_digest in [None, Some("different-final")] {
            let error = inputs.compare(&current, final_digest).unwrap_err();
            assert!(error.find_source::<StaleInputs>().is_some());
        }
        // The revision spelling is acquisition input; only its resolved commit is identity.
        current.base_revision = "another-ref-to-the-same-commit".to_owned();
        assert!(inputs.compare(&current, Some("final")).unwrap());
        // Final bytes do not bypass the identity guard exercised field-by-field above.
        current.head = "another-head".to_owned();
        let error = inputs.compare(&current, Some("final")).unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
    }

    #[test]
    fn relocated_candidates_require_final_bytes_but_not_live_root_or_revision_spelling() {
        let inputs = inputs();
        let mut candidate = inputs.clone();
        candidate.root = PathBuf::from("retained-workspace");
        candidate.base_revision.clone_from(&inputs.base);
        let error = inputs.compare_candidate(&candidate, "final").unwrap_err();
        assert!(error.find_source::<StaleInputs>().is_some());
        candidate.digest = "final".to_owned();
        inputs.compare_candidate(&candidate, "final").unwrap();
    }

    #[test]
    fn resolved_metadata_requires_the_expanded_current_schema_before_acquisition() {
        let state = ResolvedState {
            inputs: inputs(),
            files: Vec::new(),
            final_digest: "final".to_owned(),
            versions: BTreeMap::new(),
            evidence_manifest_path: "candidate/Cargo.toml".into(),
        };
        for (stage, schema) in [
            (PlanStage::Proposed, SCHEMA_VERSION),
            (PlanStage::Expanded, SCHEMA_VERSION + 1),
        ] {
            let mut plan = PlanFile::new(stage, Vec::new());
            plan.schema_version = schema;
            plan.resolved = Some(state.clone());
            let error = apply_resolved(
                &plan,
                Path::new("absent/Cargo.toml"),
                false,
                Verbose::new(false),
            )
            .unwrap_err();
            assert!(error.find_source::<ResolutionRequired>().is_some());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "canonicalizes owned manifest paths")]
    fn evidence_must_name_the_recorded_candidate_and_never_the_live_workspace() {
        let directory = tempdir().unwrap();
        let live = directory.path().join("Cargo.toml");
        let candidate = directory.path().join("candidate.toml");
        let other = directory.path().join("other.toml");
        for path in [&live, &candidate, &other] {
            fs::write(path, "").unwrap();
        }
        let mut inputs = inputs();
        inputs.root = directory.path().to_owned();
        let mut state = ResolvedState {
            inputs,
            files: Vec::new(),
            final_digest: "final".to_owned(),
            versions: BTreeMap::new(),
            evidence_manifest_path: candidate,
        };
        let error = state.verify_candidate(&other).unwrap_err();
        assert!(error.find_source::<WrongEvidenceWorkspace>().is_some());
        state.evidence_manifest_path = live.clone();
        let error = state.verify_candidate(&live).unwrap_err();
        assert!(error.find_source::<WrongEvidenceWorkspace>().is_some());
        let error = state
            .verify_candidate(&directory.path().join("absent.toml"))
            .unwrap_err();
        assert!(error.find_source::<ReadFileError>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads owned files and invokes Git hashing")]
    fn resolved_artifacts_require_exact_versions_membership_and_complete_final_bytes() {
        let directory = tempdir().unwrap();
        let mut inputs = inputs();
        inputs.root = directory.path().to_owned();
        fs::create_dir_all(directory.path().join("src")).unwrap();
        fs::write(directory.path().join("src/lib.rs"), "source").unwrap();
        fs::write(directory.path().join("Cargo.toml"), "old manifest").unwrap();
        fs::write(directory.path().join("Cargo.lock"), "old lockfile").unwrap();
        let files = vec![
            Artifact {
                path: "Cargo.toml".into(),
                contents: "new manifest".to_owned(),
            },
            Artifact {
                path: "Cargo.lock".into(),
                contents: "new lockfile".to_owned(),
            },
        ];
        let versions = BTreeMap::from([("demo".to_owned(), "0.1.1".to_owned())]);
        let allowed = BTreeSet::from([PathBuf::from("Cargo.toml"), PathBuf::from("Cargo.lock")]);
        let state = ResolvedState {
            final_digest: inputs.final_digest(&files).unwrap(),
            inputs,
            files,
            versions: versions.clone(),
            evidence_manifest_path: "not-read/Cargo.toml".into(),
        };
        state.validate_artifacts(&versions, &allowed).unwrap();
        let original = serde_json::to_value(&state).unwrap();
        let original_files = original.get("files").unwrap().as_array().unwrap();
        let mut duplicate = original_files.clone();
        duplicate.push(original_files.first().unwrap().clone());
        let mut missing = original_files.clone();
        missing.pop().unwrap();
        let mut changed_bytes = original_files.clone();
        *changed_bytes
            .first_mut()
            .unwrap()
            .get_mut("contents")
            .unwrap() = json!("other manifest");
        let mut source = original_files.clone();
        source.push(json!({"path":"src/lib.rs","contents":"unplanned source"}));
        let mut outside = original_files.clone();
        outside.push(json!({"path":"../Cargo.toml","contents":"outside"}));
        for (field, value) in [
            ("versions", json!({"demo":"9.0.0"})),
            ("final_digest", json!("wrong digest")),
            ("files", json!(duplicate)),
            ("files", json!(missing)),
            ("files", json!(changed_bytes)),
            ("files", json!(source)),
            ("files", json!(outside)),
        ] {
            let mut changed: Value = original.clone();
            *changed.get_mut(field).unwrap() = value;
            let changed: ResolvedState = serde_json::from_value(changed).unwrap();
            let error = changed.validate_artifacts(&versions, &allowed).unwrap_err();
            assert!(error.find_source::<ResolutionRequired>().is_some());
        }
        let mut uncaptured = state.clone();
        uncaptured.inputs.paths.remove(Path::new("Cargo.lock"));
        let error = uncaptured
            .validate_artifacts(&versions, &allowed)
            .unwrap_err();
        assert!(error.find_source::<ResolutionRequired>().is_some());
        let mut unowned = allowed;
        unowned.remove(Path::new("Cargo.lock"));
        let error = state.validate_artifacts(&versions, &unowned).unwrap_err();
        assert!(error.find_source::<ResolutionRequired>().is_some());

        // Once the live bytes already match, an empty resolved write set is complete.
        for file in &state.files {
            fs::write(state.inputs.root.join(&file.path), &file.contents).unwrap();
        }
        let state = ResolvedState {
            files: Vec::new(),
            versions: BTreeMap::new(),
            ..state
        };
        state
            .validate_artifacts(&BTreeMap::new(), &BTreeSet::new())
            .unwrap();
    }

    #[test]
    fn fields_use_fixed_width_little_endian_lengths() {
        let mut bytes = Vec::new();
        append_field(&mut bytes, b"abc");
        append_field(&mut bytes, b"");
        assert_eq!(
            bytes,
            [
                3, 0, 0, 0, 0, 0, 0, 0, b'a', b'b', b'c', 0, 0, 0, 0, 0, 0, 0, 0
            ]
        );
    }

    #[test]
    fn field_boundaries_participate_in_fingerprints() {
        let mut left = Vec::new();
        append_field(&mut left, b"ab");
        append_field(&mut left, b"c");
        let mut right = Vec::new();
        append_field(&mut right, b"a");
        append_field(&mut right, b"bc");
        assert_ne!(left, right);
    }

    #[test]
    #[cfg(windows)]
    fn canonical_windows_paths_match_git_and_cargo_spellings() {
        for (canonical, ordinary) in [
            (
                r"\\?\UNC\server\share\workspace",
                r"\\server\share\workspace",
            ),
            (r"\\?\C:\workspace", r"C:\workspace"),
            (r"C:\workspace", r"C:\workspace"),
        ] {
            assert_eq!(
                ordinary_windows_path(Path::new(canonical)),
                Path::new(ordinary)
            );
        }
    }

    #[test]
    fn artifact_selection_rejects_uncaptured_non_cargo_and_duplicate_paths() {
        let inputs = Inputs {
            root: PathBuf::from("not-accessed"),
            manifest: PathBuf::from("Cargo.toml"),
            head: String::new(),
            base: String::new(),
            base_revision: String::new(),
            index: String::new(),
            paths: ["Cargo.toml", "src/lib.rs"]
                .into_iter()
                .map(PathBuf::from)
                .collect(),
            digest: String::new(),
        };
        for paths in [
            vec!["uncaptured/Cargo.toml"],
            vec!["src/lib.rs"],
            vec!["Cargo.toml", "Cargo.toml"],
        ] {
            let artifacts: Vec<_> = paths
                .into_iter()
                .map(|path| Artifact {
                    path: path.into(),
                    contents: String::new(),
                })
                .collect();
            let error = inputs.final_digest(&artifacts).unwrap_err();
            assert!(error.find_source::<ResolutionRequired>().is_some());
        }
    }

    #[test]
    fn relative_paths_cannot_escape_the_captured_root() {
        let root = Path::new("repository");
        assert_eq!(
            relative(root, &root.join("member/Cargo.toml")).unwrap(),
            Path::new("member/Cargo.toml")
        );
        for path in [
            PathBuf::from("elsewhere/Cargo.toml"),
            root.join("../Cargo.toml"),
        ] {
            let error = relative(root, &path).unwrap_err();
            assert!(error.find_source::<UnsupportedInput>().is_some());
        }
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads an owned filesystem fixture")]
    fn source_collection_handles_absence_recursion_and_non_directory_errors() {
        let directory = tempdir().unwrap();
        let source = directory.path().join("src");
        let mut paths = BTreeSet::new();
        collect_sources(directory.path(), &source, &mut paths).unwrap();
        assert!(paths.is_empty());
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("lib.rs"), "").unwrap();
        fs::write(source.join("nested/mod.rs"), "").unwrap();
        collect_sources(directory.path(), &source, &mut paths).unwrap();
        assert_eq!(
            paths,
            ["src/lib.rs", "src/nested/mod.rs"]
                .into_iter()
                .map(PathBuf::from)
                .collect()
        );
        let error =
            collect_sources(directory.path(), &source.join("lib.rs"), &mut paths).unwrap_err();
        assert!(error.find_source::<ReadFileError>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads files and invokes Git hashing")]
    fn fingerprints_distinguish_missing_empty_content_and_paths() {
        let directory = tempdir().unwrap();
        let path = PathBuf::from("Cargo.lock");
        let paths = BTreeSet::from([path.clone()]);
        let missing = fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap();
        fs::write(directory.path().join(&path), "").unwrap();
        let empty = fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap();
        assert_ne!(missing, empty);
        let contents = b"resolved bytes".to_vec();
        let replacements = BTreeMap::from([(path.clone(), contents.clone())]);
        let replaced = fingerprint(directory.path(), &paths, &replacements).unwrap();
        assert_ne!(empty, replaced);
        fs::write(directory.path().join(&path), &contents).unwrap();
        assert_eq!(
            fingerprint(directory.path(), &paths, &BTreeMap::new()).unwrap(),
            replaced
        );
        fs::rename(
            directory.path().join(path),
            directory.path().join("Cargo.toml"),
        )
        .unwrap();
        let renamed = fingerprint(
            directory.path(),
            &BTreeSet::from([PathBuf::from("Cargo.toml")]),
            &BTreeMap::new(),
        )
        .unwrap();
        assert_ne!(renamed, replaced);
    }

    #[test]
    #[cfg_attr(miri, ignore = "uses filesystem metadata")]
    fn fingerprints_reject_a_directory_as_a_file() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("Cargo.lock")).unwrap();
        let error = fingerprint(
            directory.path(),
            &BTreeSet::from([PathBuf::from("Cargo.lock")]),
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert!(error.find_source::<UnsupportedInput>().is_some());
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads local dependency manifests")]
    fn capture_includes_workspace_and_replacement_sources() {
        let directory = tempdir().unwrap();
        // The traversal consumes the canonical root captured by Inputs.
        let root = canonical(directory.path()).unwrap();
        let root = root.as_path();
        fs::create_dir_all(root.join("replacement/src/nested")).unwrap();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace.dependencies]\nhelper = { path = \"replacement\" }\n\
             [replace]\n\"helper:0.1.0\" = { path = \"replacement\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("replacement/Cargo.toml"),
            "[package]\nname = \"helper\"\nversion = \"0.1.0\"\n",
        )
        .unwrap();
        fs::write(root.join("replacement/src/nested/lib.rs"), "").unwrap();
        let mut paths = BTreeSet::new();
        capture_path_dependencies(root, [&root.join("Cargo.toml")], &mut paths).unwrap();
        assert_eq!(
            paths,
            [
                "Cargo.toml",
                "replacement/Cargo.toml",
                "replacement/src/nested/lib.rs"
            ]
            .into_iter()
            .map(PathBuf::from)
            .collect()
        );
    }

    #[test]
    #[cfg_attr(miri, ignore = "reads an owned dependency manifest")]
    fn absolute_dependency_paths_cannot_leak_back_to_the_live_workspace() {
        let directory = tempdir().unwrap();
        let manifest = directory.path().join("Cargo.toml");
        fs::write(
            &manifest,
            format!(
                "[dependencies]\nhelper = {{ path = {:?} }}\n",
                directory.path()
            ),
        )
        .unwrap();
        let error = capture_path_dependencies(directory.path(), [&manifest], &mut BTreeSet::new())
            .unwrap_err();
        assert!(error.find_source::<UnsupportedInput>().is_some());
    }

    #[test]
    fn unsupported_artifact_schema_precedes_body_validation() {
        let error =
            parse_artifact::<PlanFile>(Path::new("prepared.json"), r#"{"schema_version":3}"#)
                .unwrap_err();
        assert!(error.find_source::<UnsupportedPlanSchemaError>().is_some());
    }
}
