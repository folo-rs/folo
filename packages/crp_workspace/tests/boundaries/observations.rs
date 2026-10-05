//! Fresh workspace acquisition and immutable Git interpretation at native boundaries.

use std::fs;

use crp_workspace::git::{CommitHeaders, GitObjectContext, HistoricalTree};
use crp_workspace::manifest_document::ManifestDocuments;
use crp_workspace::metadata::{load_tracked_work_tree, load_tracked_work_tree_with_documents};
use crp_workspace::source_inputs::SourceInputs;

use crate::git_fixture::Repository;

fn repository() -> Repository {
    let fixture = Repository::new();
    fixture.write("Cargo.toml", b"[workspace]\nmembers=[]\n");
    fixture.command(&["add", "Cargo.toml"]);
    fixture.command(&["commit", "--quiet", "-m", "root"]);
    fixture
}

#[test]
#[cfg_attr(
    miri,
    ignore = "acquires Cargo metadata and current tracked paths across mutations"
)]
fn parsed_members_reinterpret_changed_workspace_context_and_relocate_without_stale_paths() {
    let first = repository();
    for fixture in [first, repository()] {
        let manifest = fixture.path().join("Cargo.toml");
        let mut documents = ManifestDocuments::default();
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
            let (observed, _) =
                load_tracked_work_tree_with_documents(&manifest, &mut documents).unwrap();
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
            assert_eq!(
                observed.tracked_paths.as_ref(),
                fixture.repo().ls_files("").unwrap()
            );
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

#[test]
#[cfg_attr(miri, ignore = "acquires native Git replacement observations")]
fn replacements_do_not_reuse_original_object_facts_and_removal_restores_original_interpretation() {
    let fixture = repository();
    let git = fixture.repo();
    let root = git.rev_parse("HEAD").unwrap();
    let original = GitObjectContext::capture(&git).unwrap();
    let tree = HistoricalTree::load(&git, &root, &original).unwrap();
    assert!(tree.entry("changed").is_none());
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &original)
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
        HistoricalTree::load(&git, &root, &replaced)
            .unwrap()
            .entry("changed")
            .is_some()
    );
    assert!(
        CommitHeaders::default()
            .has_parent(&git, &root, &replaced)
            .unwrap()
    );
    fixture.command(&["replace", "-d", &root]);
    let restored = GitObjectContext::capture(&git).unwrap();
    assert_eq!(original, restored);
    assert!(
        HistoricalTree::load(&git, &root, &restored)
            .unwrap()
            .entry("changed")
            .is_none()
    );
    assert!(
        !CommitHeaders::default()
            .has_parent(&git, &root, &restored)
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
    let git = fixture.repo();
    let context = GitObjectContext::capture(&git).unwrap();
    let parent = CommitHeaders::default()
        .has_parent(&git, &head, &context)
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
            .has_parent(&git, &head, &context)
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
