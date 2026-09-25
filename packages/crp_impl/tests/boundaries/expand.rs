//! External acquisition for expand.

use std::fs;
use std::path::Path;

use crp_impl::WriteFileError;
use crp_impl::expand::*;
use tempfile::{NamedTempFile, tempdir};

#[test]
#[cfg_attr(miri, ignore = "creates missing output directories")]
fn expansion_creates_missing_output_parents_in_both_modes() {
    let directory = tempdir().unwrap();
    for preserve_input in [false, true] {
        let output = directory
            .path()
            .join(preserve_input.to_string())
            .join("nested")
            .join("expanded.json");
        write_expansion(&output, "complete", preserve_input).unwrap();
        assert_eq!(fs::read_to_string(output).unwrap(), "complete");
    }
}

#[test]
#[cfg_attr(miri, ignore = "stages files beside the requested output")]
fn staging_is_complete_and_beside_the_untouched_destination() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("expanded.json");
    fs::write(&output, "previous").unwrap();
    let staged = stage_expansion(&output, "complete").unwrap();
    assert_eq!(
        fs::canonicalize(staged.path().parent().unwrap()).unwrap(),
        fs::canonicalize(directory.path()).unwrap()
    );
    assert_ne!(staged.path(), output);
    assert_eq!(fs::read_to_string(staged.path()).unwrap(), "complete");
    assert_eq!(fs::read_to_string(&output).unwrap(), "previous");
    drop(staged);
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(miri, ignore = "stages files on the host filesystem")]
fn protected_writes_promote_complete_output_and_remove_staging_files() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("expanded.json");
    fs::write(&output, "previous").unwrap();
    write_expansion(&output, "complete", true).unwrap();
    assert_eq!(fs::read_to_string(&output).unwrap(), "complete");
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(miri, ignore = "exercises a failed filesystem promotion")]
fn failed_promotion_preserves_the_destination_and_cleans_staging() {
    let directory = tempdir().unwrap();
    let output = directory.path().join("occupied");
    fs::create_dir_all(&output).unwrap();
    fs::write(output.join("input"), "retained").unwrap();
    let error = write_expansion(&output, "complete", true).unwrap_err();
    assert!(error.find_source::<WriteFileError>().is_some());
    assert_eq!(
        fs::read_to_string(output.join("input")).unwrap(),
        "retained"
    );
    assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
#[cfg_attr(
    miri,
    ignore = "uses an exclusively owned file in the current directory"
)]
fn protected_writes_support_bare_filenames() {
    let owned = NamedTempFile::new_in(".").unwrap().into_temp_path();
    let output = Path::new(owned.file_name().unwrap());
    write_expansion(output, "complete", true).unwrap();
    assert_eq!(fs::read_to_string(output).unwrap(), "complete");
}
