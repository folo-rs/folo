//! External acquisition for prospective.

use std::fs;
use std::path::Path;

use crp_impl::WriteFileError;
use crp_impl::prospective::*;
use crp_impl::resolved::canonical;
use crp_impl::verbose::Verbose;
use tempfile::tempdir;

fn candidate(root: &Path) -> Prospective {
    fs::create_dir_all(root).unwrap();
    let manifest = root.join("nested/Cargo.toml");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    fs::write(&manifest, "captured manifest").unwrap();
    Prospective {
        root: root.to_path_buf(),
        manifest,
        retained: false,
    }
}

fn resolver_candidate(root: &Path) -> Prospective {
    let prospective = candidate(root);
    fs::write(
        &prospective.manifest,
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\
             [workspace]\n",
    )
    .unwrap();
    let package = prospective.manifest.parent().unwrap();
    fs::create_dir_all(package.join("src")).unwrap();
    fs::write(package.join("src/lib.rs"), "pub fn released() {}\n").unwrap();
    prospective
}

#[test]
#[cfg_attr(
    miri,
    ignore = "resolves a real Cargo manifest under a filesystem case alias"
)]
fn resolver_preserves_captured_spelling_at_the_cargo_boundary() {
    let directory = tempdir().unwrap();
    let prospective = resolver_candidate(&directory.path().join("workspace"));
    let package = prospective.manifest.parent().unwrap();
    let alias = package.join("cargo.toml");
    if !alias.exists() {
        eprintln!("This filesystem does not provide a case alias for Cargo.toml.");
        return;
    }
    let intermediate = package.join("case-rename");
    fs::rename(&prospective.manifest, &intermediate).unwrap();
    fs::rename(intermediate, &alias).unwrap();
    let mut prospective = prospective;
    prospective.manifest = canonical(&alias).unwrap();
    prospective.resolve(Verbose::new(false)).unwrap();
    assert_eq!(prospective.manifest.file_name().unwrap(), "cargo.toml");
    assert!(
        fs::read_to_string(alias.with_file_name("Cargo.lock"))
            .unwrap()
            .contains("name = \"demo\"")
    );
}

#[test]
#[cfg_attr(miri, ignore = "invokes Cargo with an unsupported manifest filename")]
fn resolver_rejects_an_unrelated_manifest_filename() {
    let directory = tempdir().unwrap();
    let mut prospective = resolver_candidate(&directory.path().join("workspace"));
    let unrelated = prospective.manifest.with_file_name("manifest.input");
    fs::copy(&prospective.manifest, &unrelated).unwrap();
    prospective.manifest = unrelated;
    prospective.resolve(Verbose::new(false)).unwrap_err();
    assert!(!prospective.manifest.with_file_name("Cargo.lock").exists());
}

#[test]
#[cfg_attr(miri, ignore = "moves an owned filesystem fixture")]
fn retain_preserves_the_manifest_location_and_replaces_only_its_owner() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("output");
    fs::create_dir_all(&output).unwrap();
    let owner = directory.path().join("original");
    let prospective = candidate(&directory.path().join("candidate"));
    fs::create_dir_all(prospective.root.join(".git")).unwrap();
    let manifest = prospective.retain(&output, &owner).unwrap();
    assert_eq!(manifest, output.join("workspace/nested/Cargo.toml"));
    assert_eq!(fs::read_to_string(&manifest).unwrap(), "captured manifest");
    assert_eq!(
        fs::read_to_string(output.join("workspace").join(EVIDENCE_MARKER)).unwrap(),
        owner.to_string_lossy()
    );

    let obsolete = output.join("workspace/old-evidence");
    fs::write(&obsolete, "previous candidate only").unwrap();
    let prospective = candidate(&directory.path().join("replacement"));
    fs::create_dir_all(prospective.root.join(".git")).unwrap();
    prospective.retain(&output, &owner).unwrap();
    assert!(!obsolete.exists());
    let prospective = candidate(&directory.path().join("foreign"));
    let error = prospective
        .retain(&output, &directory.path().join("another-owner"))
        .unwrap_err();
    assert!(error.find_source::<EvidenceWorkspaceOccupied>().is_some());
    assert_eq!(fs::read_to_string(&manifest).unwrap(), "captured manifest");

    fs::remove_file(output.join("workspace").join(EVIDENCE_MARKER)).unwrap();
    let unmarked = directory.path().join("unmarked");
    let prospective = candidate(&unmarked);
    let error = prospective.retain(&output, &owner).unwrap_err();
    assert!(error.find_source::<EvidenceWorkspaceOccupied>().is_some());
    assert_eq!(fs::read_to_string(manifest).unwrap(), "captured manifest");
    assert!(!unmarked.exists());
}

#[test]
#[cfg_attr(miri, ignore = "exercises owned filesystem failure paths")]
fn retaining_failure_cleans_the_candidate_without_creating_evidence() {
    let directory = tempdir().unwrap();
    let owner = directory.path().join("original");
    for marker_available in [false, true] {
        let root = directory.path().join("candidate");
        let prospective = candidate(&root);
        if marker_available {
            fs::create_dir_all(root.join(".git")).unwrap();
        }
        // A missing marker directory fails before rename. A missing destination parent
        // fails after marker creation. Both errors must discard only the owned candidate.
        let output = directory.path().join("absent-parent");
        let error = prospective.retain(&output, &owner).unwrap_err();
        assert!(error.find_source::<WriteFileError>().is_some());
        assert!(!root.exists());
        assert!(!output.exists());
    }
}
