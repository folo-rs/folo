//! Immutable observation reuse and live Git interpretation at the native boundary.

#[cfg(unix)]
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;
#[cfg(windows)]
use std::process::Command;
use std::sync::Mutex;
use std::{fs, io};

use crp_diag::{DiagnosticSink, Discard, Verbose};
use crp_workspace::cache::{Cache, CacheOptions};
use crp_workspace::git::{CommitHeaders, GitObjectContext, HistoricalTree};
use crp_workspace::lockfile::Lockfile;
use crp_workspace::manifest::PathCase;
use crp_workspace::manifest_document::ManifestDocuments;
use crp_workspace::metadata::{load_tracked_work_tree, load_tracked_work_tree_with_documents};
use crp_workspace::source_inputs::SourceInputs;
use tempfile::TempDir;

use crate::git_fixture::Repository;

fn repository() -> Repository {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\nmembers=[]\n");
    fixture.command(&["add", "Cargo.toml"]);
    fixture.command(&["commit", "--quiet", "-m", "root"]);
    fixture
}

/// Captures subject acquisitions and advisory storage failures at the native boundary.
#[derive(Debug, Default)]
struct Recording(Mutex<String>);

impl DiagnosticSink for Recording {
    fn write(&self, text: &str) -> io::Result<()> {
        self.0.lock().unwrap().push_str(text);
        Ok(())
    }
}

#[test]
#[cfg_attr(
    miri,
    ignore = "exercises typed cache persistence and filesystem corruption"
)]
fn parsed_subjects_reuse_across_owners_and_recover_from_corruption() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    let storage = directory.path().join("cache");
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(storage.clone()),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    let sink = Recording::default();
    let verbose = Verbose::new(true, &sink);
    let text = "[workspace]\nmembers=['member']\n[workspace.dependencies]\na = '= 1.2.3'\n";
    let lock = "version=4\n[[package]]\nname='root'\nversion='1.0.0'\n";
    for iteration in 0..2 {
        let doc = ManifestDocuments::new(cache.clone())
            .parse(
                Path::new(if iteration == 0 {
                    "original/Cargo.toml"
                } else {
                    "relocated/Cargo.toml"
                }),
                text,
                verbose,
            )
            .unwrap();
        assert_eq!(
            doc.get("workspace")
                .unwrap()
                .get("dependencies")
                .unwrap()
                .get("a")
                .unwrap()
                .as_str(),
            Some("= 1.2.3")
        );
        let parsed = Lockfile::parse_cached(lock, "Cargo.lock", &cache, verbose).unwrap();
        assert_eq!(parsed.entries.first().unwrap().name, "root");
    }
    let output = sink.0.lock().unwrap().clone();
    assert_eq!(output.matches("acquiring manifest-document ").count(), 1);
    assert_eq!(output.matches("acquiring lockfile ").count(), 1);

    let changed = text.replace("= 1.2.3", "=1.2.4");
    let document = ManifestDocuments::new(cache.clone())
        .parse(Path::new("relocated/Cargo.toml"), &changed, verbose)
        .unwrap();
    assert_eq!(
        document
            .get("workspace")
            .unwrap()
            .get("dependencies")
            .unwrap()
            .get("a")
            .unwrap()
            .as_str(),
        Some("=1.2.4")
    );
    let changed_lock = lock.replace("1.0.0", "1.0.1");
    let parsed = Lockfile::parse_cached(&changed_lock, "Cargo.lock", &cache, verbose).unwrap();
    assert_eq!(parsed.entries.first().unwrap().version.to_string(), "1.0.1");
    assert_eq!(
        sink.0
            .lock()
            .unwrap()
            .matches("acquiring lockfile ")
            .count(),
        2
    );
    assert_eq!(
        sink.0
            .lock()
            .unwrap()
            .matches("acquiring manifest-document ")
            .count(),
        2
    );

    for subject in ["manifest-document", "lockfile"] {
        for entry in fs::read_dir(storage.join(subject)).unwrap() {
            fs::write(entry.unwrap().path(), b"truncated").unwrap();
        }
    }
    let recovered = ManifestDocuments::new(cache.clone())
        .parse(Path::new("third/Cargo.toml"), text, verbose)
        .unwrap();
    assert_eq!(
        recovered
            .get("workspace")
            .unwrap()
            .get("dependencies")
            .unwrap()
            .get("a")
            .unwrap()
            .as_str(),
        Some("= 1.2.3")
    );
    let recovered = Lockfile::parse_cached(lock, "Cargo.lock", &cache, verbose).unwrap();
    let fresh = Lockfile::parse(lock, "Cargo.lock").unwrap();
    assert_eq!(
        serde_json::to_value(recovered).unwrap(),
        serde_json::to_value(fresh).unwrap()
    );
    assert_eq!(
        sink.0
            .lock()
            .unwrap()
            .matches("continuing with fresh observations")
            .count(),
        1
    );
}

#[test]
#[cfg_attr(
    miri,
    ignore = "acquires Cargo metadata and current tracked paths across mutations"
)]
fn parsed_members_reinterpret_changed_workspace_context_and_relocate_without_stale_paths() {
    let storage = TempDir::new().unwrap();
    let first = repository();
    let cache = Cache::resolve(
        &first.path().join("Cargo.toml"),
        &CacheOptions::Directory(storage.path().join("cache")),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    for fixture in [first, repository()] {
        let manifest = fixture.path().join("Cargo.toml");
        let mut documents = ManifestDocuments::new(cache.clone());
        for (member, version) in [("old", "1.0.0"), ("new", "2.0.0")] {
            fixture.write(
                "Cargo.toml",
                format!(
                    "[workspace]\nmembers=['{member}']\n[workspace.package]\nversion='{version}'\n",
                )
                .as_bytes(),
            );
            fixture.write(
                &format!("{member}/Cargo.toml"),
                b"[package]\nname='member'\nversion.workspace=true\n[lib]\npath='lib.rs'\n",
            );
            fixture.write(&format!("{member}/lib.rs"), b"pub fn library() {}\n");
            fixture.command(&["add", "."]);
            let (observed, _) = load_tracked_work_tree_with_documents(
                &manifest,
                &mut documents,
                Verbose::new(false, &Discard),
            )
            .unwrap();
            let (fresh, _) = load_tracked_work_tree(&manifest).unwrap();
            assert_eq!(observed.target_versions(), fresh.target_versions());
            assert_eq!(observed.member_manifests, fresh.member_manifests);
            assert_eq!(
                observed
                    .packages
                    .first()
                    .unwrap()
                    .manifest
                    .version
                    .to_string(),
                version
            );
            assert_eq!(
                observed.packages.first().unwrap().manifest.directory,
                member
            );
            assert!(
                observed
                    .member_manifests
                    .iter()
                    .all(|path| path.starts_with(&observed.workspace_root))
            );
            assert!(
                observed
                    .manifests
                    .documents
                    .keys()
                    .all(|path| path.starts_with(&observed.workspace_root))
            );
            assert_eq!(observed.tracked_paths, fixture.repo().ls_files("").unwrap());
            assert!(
                observed
                    .tracked_paths
                    .contains(&format!("{member}/Cargo.toml"))
            );
            let git = fixture.repo();
            let shared_sources = SourceInputs::discover_with_documents(
                git.root(),
                &observed.workspace_root,
                &observed.member_manifests,
                |_, _| panic!("this fixture has no path dependencies"),
                |path| Ok(observed.manifests.documents.get(path).unwrap().clone()),
            )
            .unwrap();
            let fresh_sources = SourceInputs::discover(
                git.root(),
                &fresh.workspace_root,
                &fresh.member_manifests,
                |_, _| panic!("this fixture has no path dependencies"),
            )
            .unwrap();
            assert_eq!(shared_sources.files, fresh_sources.files);
            assert_eq!(
                shared_sources.source_directories,
                fresh_sources.source_directories
            );
            assert!(
                shared_sources
                    .files
                    .contains(&observed.workspace_root.join(member).join("build.rs"))
            );
        }
    }
}

// Junctions need no symbolic-link privilege on Windows and exercise its reparse-point boundary.
fn link_directory(target: &Path, link: &Path) {
    #[cfg(unix)]
    symlink(target, link).unwrap();
    #[cfg(windows)]
    {
        let output = Command::new("pwsh")
            .args([
                "-NoProfile",
                "-Command",
                "New-Item -ItemType Junction -Path $env:CRP_LINK -Target $env:CRP_TARGET -ErrorAction Stop | Out-Null",
            ])
            .env("CRP_LINK", link)
            .env("CRP_TARGET", target)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[cfg_attr(miri, ignore = "creates native symlinks or directory junctions")]
fn cache_protects_intermediate_evidence_link_entries() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let storage = directory.path().join("cache");
    fs::create_dir_all(&storage).unwrap();
    let link = storage.join("linked");
    link_directory(target.path(), &link);
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(storage),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    assert!(cache.protect(&link.join("proposal.json")).is_err());
}

#[test]
#[cfg_attr(miri, ignore = "creates redirected native cache subject directories")]
fn redirected_subject_directories_never_receive_cache_entries() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    let target = TempDir::new().unwrap();
    let storage = directory.path().join("cache");
    fs::create_dir_all(&storage).unwrap();
    link_directory(target.path(), &storage.join("git-trees"));
    let verbose = Verbose::new(false, &Discard);
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(storage),
        verbose,
    )
    .unwrap();
    let git = fixture.repo();
    let commit = git.rev_parse("HEAD").unwrap();
    let context = GitObjectContext::capture(&git).unwrap();
    let tree = HistoricalTree::load(&git, &commit, &context, &cache, verbose).unwrap();
    assert!(tree.entry("Cargo.toml").is_some());
    assert_eq!(fs::read_dir(target.path()).unwrap().count(), 0);
    let warm = TempDir::new().unwrap();
    let admitted = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(warm.path().to_owned()),
        verbose,
    )
    .unwrap();
    HistoricalTree::load(&git, &commit, &context, &admitted, verbose).unwrap();
    let entry = fs::read_dir(warm.path().join("git-trees"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    fs::copy(entry.path(), target.path().join(entry.file_name())).unwrap();
    fs::rename(
        fixture.path().join(".git/objects"),
        warm.path().join("original-objects"),
    )
    .unwrap();
    // A valid entry behind the redirect must not conceal a genuine acquisition failure.
    HistoricalTree::load(&git, &commit, &context, &cache, verbose).unwrap_err();
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "creates tracked source and evidence symlink entries")]
fn cache_protects_source_and_leaf_evidence_symlink_entries() {
    let fixture = repository();
    let target = TempDir::new().unwrap();
    fs::write(target.path().join("proposal.json"), "{}").unwrap();
    fs::create_dir_all(fixture.path().join("docs")).unwrap();
    symlink(target.path(), fixture.path().join("docs/link")).unwrap();
    fixture.command(&["add", "docs/link"]);
    fixture.command(&["commit", "--quiet", "-m", "source link"]);
    Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(fixture.path().join("docs")),
        Verbose::new(false, &Discard),
    )
    .unwrap_err();
    let directory = TempDir::new().unwrap();
    let marker = directory.path().join("proposal.json");
    symlink(target.path().join("proposal.json"), &marker).unwrap();
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().to_owned()),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    assert!(cache.protect(&marker).is_err());
}

#[test]
#[cfg_attr(miri, ignore = "resolves native Git administration and path aliases")]
fn administration_and_aliased_reserved_inputs_are_not_disposable_storage() {
    let fixture = repository();
    let linked = TempDir::new().unwrap();
    fixture.command(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        linked.path().to_str().unwrap(),
        "HEAD",
    ]);
    for directory in [
        linked.path().join(".git"),
        fixture.path().join(".git"),
        fixture.path().join(".git/unused-cache"),
    ] {
        let error = Cache::resolve(
            &linked.path().join("Cargo.toml"),
            &CacheOptions::Directory(directory),
            Verbose::new(false, &Discard),
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlaps"));
    }
    if PathCase::probe(fixture.path()) == PathCase::Insensitive {
        let alias = fixture.path().to_string_lossy().to_uppercase();
        let error = Cache::resolve(
            &Path::new(&alias).join("Cargo.toml"),
            &CacheOptions::Directory(fixture.path().join(".cargo/config.toml")),
            Verbose::new(false, &Discard),
        )
        .unwrap_err();
        assert!(error.to_string().contains("overlaps"));
    }
}

#[test]
#[cfg(unix)]
#[cfg_attr(miri, ignore = "probes native read-only directory case rules")]
fn evidence_case_admission_uses_existing_entries_in_read_only_directories() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    fs::write(directory.path().join("probe.txt"), "").unwrap();
    let case = PathCase::probe(directory.path());
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    let original = fs::metadata(directory.path()).unwrap().permissions();
    // Remove all write bits while retaining the fixture's existing read/search permissions.
    fs::set_permissions(
        directory.path(),
        fs::Permissions::from_mode(original.mode() & !0o222),
    )
    .unwrap();
    let result = cache.protect(&directory.path().join("CACHE/report"));
    fs::set_permissions(directory.path(), original).unwrap();
    assert_eq!(result.is_ok(), case == PathCase::Sensitive);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "acquires native Git replacement and cache observations"
)]
fn replacements_do_not_reuse_original_object_facts_and_removal_restores_eligibility() {
    let fixture = repository();
    let directory = TempDir::new().unwrap();
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    let git = fixture.repo();
    let root = git.rev_parse("HEAD").unwrap();
    let verbose = Verbose::new(false, &Discard);
    let original = GitObjectContext::capture(&git).unwrap();
    let tree = HistoricalTree::load(&git, &root, &original, &cache, verbose).unwrap();
    assert!(tree.entry("changed").is_none());
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &original, &cache, verbose)
            .unwrap()
    );

    fixture.write("changed", b"different tree");
    fixture.command(&["add", "changed"]);
    fixture.command(&["commit", "--quiet", "-m", "child"]);
    let child = git.rev_parse("HEAD").unwrap();
    fixture.command(&["replace", &root, &child]);
    let replaced = GitObjectContext::capture(&git).unwrap();
    assert_ne!(original, replaced);
    assert!(
        HistoricalTree::load(&git, &root, &replaced, &cache, verbose)
            .unwrap()
            .entry("changed")
            .is_some()
    );
    assert!(
        CommitHeaders::default()
            .has_parent(&git, &root, &replaced, &cache, verbose)
            .unwrap()
    );
    fixture.command(&["replace", "-d", &root]);
    let restored = GitObjectContext::capture(&git).unwrap();
    assert_eq!(original, restored);
    assert!(
        HistoricalTree::load(&git, &root, &restored, &cache, verbose)
            .unwrap()
            .entry("changed")
            .is_none()
    );
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &restored, &cache, verbose)
            .unwrap()
    );
}

#[test]
#[cfg_attr(miri, ignore = "changes local graft and shallow history observations")]
fn graft_context_changes_and_cached_headers_do_not_freeze_shallow_traversal() {
    let source = repository();
    let root = source.repo().rev_parse("HEAD").unwrap();
    source.command(&["commit", "--quiet", "--allow-empty", "-m", "child"]);
    let head = source.repo().rev_parse("HEAD").unwrap();
    let fixture = Repository::new();
    fixture.command(&[
        "fetch",
        "--quiet",
        "--depth=1",
        &source.path().to_string_lossy(),
        "HEAD",
    ]);
    fixture.command(&["checkout", "--quiet", "--detach", "FETCH_HEAD"]);
    let directory = TempDir::new().unwrap();
    let cache = Cache::resolve(
        &fixture.path().join("Cargo.toml"),
        &CacheOptions::Directory(directory.path().join("cache")),
        Verbose::new(false, &Discard),
    )
    .unwrap();
    let git = fixture.repo();
    let context = GitObjectContext::capture(&git).unwrap();
    let verbose = Verbose::new(false, &Discard);
    let parent = CommitHeaders::default()
        .has_parent(&git, &head, &context, &cache, verbose)
        .unwrap();
    assert!(parent);
    assert!(git.parent_boundary_with_header(&head, parent).unwrap());
    assert_eq!(
        git.first_parent_commits(&head).unwrap(),
        std::slice::from_ref(&head)
    );
    fixture.command(&[
        "fetch",
        "--quiet",
        "--unshallow",
        &source.path().to_string_lossy(),
        "HEAD",
    ]);
    assert_eq!(GitObjectContext::capture(&git).unwrap(), context);
    assert!(
        CommitHeaders::default()
            .has_parent(&git, &head, &context, &cache, verbose)
            .unwrap()
    );
    assert_eq!(
        git.first_parent_commits(&head).unwrap(),
        [head.clone(), root]
    );
    assert!(!git.is_shallow().unwrap());

    let graft = fixture.path().join(".git").join("info").join("grafts");
    fs::create_dir_all(graft.parent().unwrap()).unwrap();
    fs::write(&graft, format!("{head}\n")).unwrap();
    assert_ne!(GitObjectContext::capture(&git).unwrap(), context);
    fs::remove_file(&graft).unwrap();
    assert_eq!(GitObjectContext::capture(&git).unwrap(), context);
}
